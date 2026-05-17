//! Smithay-based nested compositor: socket, globals, dispatch loop.
//!
//! v1 scope: single client, single fullscreen toplevel, shm buffers,
//! keyboard + pointer + scroll, wl_output advertisement, xdg_activation
//! stub. No popups, no subsurfaces, no dmabuf/EGL — clients that hard-
//! require GPU (Chromium-based: helium-browser, Electron) won't render
//! until linux-dmabuf-v1 + GBM are wired up. That's a separate effort.

use std::io;
use std::os::unix::io::AsRawFd;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use calloop::{
    generic::{FdWrapper, Generic},
    timer::{TimeoutAction, Timer},
    EventLoop, Interest, Mode as CMode, PostAction,
};

use smithay::{
    delegate_compositor, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_activation, delegate_xdg_shell,
    input::{
        keyboard::{FilterResult, KeyboardHandle, KeysymHandle, ModifiersState, XkbConfig},
        pointer::{
            AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent, PointerHandle,
        },
        Seat, SeatHandler, SeatState,
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{wl_buffer, wl_seat, wl_shm, wl_surface::WlSurface},
        Client, Display,
    },
    utils::{Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_states, with_surface_tree_downward, BufferAssignment,
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction,
        },
        output::{OutputHandler, OutputManagerState},
        selection::SelectionHandler,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{with_buffer_contents, ShmHandler, ShmState},
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
        socket::ListeningSocketSource,
    },
    xwayland::{XWayland, XWaylandEvent},
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;

use crate::{input::InputCmd, sink::Frame};

// ─── State ────────────────────────────────────────────────────────────────────

pub struct State {
    pub compositor_state:  CompositorState,
    pub xdg_shell_state:   XdgShellState,
    pub shm_state:         ShmState,
    pub seat_state:        SeatState<Self>,
    pub xdg_activation:    XdgActivationState,
    pub output_manager:    OutputManagerState,
    pub seat:              Seat<Self>,
    pub keyboard:          KeyboardHandle<Self>,
    pub pointer:           PointerHandle<Self>,
    pub output:            Output,
    pub output_w:          u32,
    pub output_h:          u32,
    /// Last absolute pointer position; pointer.motion() needs an absolute
    /// location, so we keep track of it across button/scroll events.
    pub pointer_pos:       (f64, f64),
    pub toplevels:         Vec<ToplevelSurface>,
    pub frame_tx:          mpsc::Sender<Frame>,
    pub serial_counter:    u32,
    pub frame_serial:      u64,
    pub running:           bool,
}

impl State {
    fn next_serial(&mut self) -> Serial {
        self.serial_counter = self.serial_counter.wrapping_add(1);
        Serial::from(self.serial_counter)
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _id: ClientId) { tracing::debug!("client connected"); }
    fn disconnected(&self, _id: ClientId, _r: DisconnectReason) { tracing::debug!("client gone"); }
}

// ─── Handler impls ────────────────────────────────────────────────────────────

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState { &mut self.compositor_state }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        let buffer_opt = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let attrs = guard.current();
            match attrs.buffer.take() {
                Some(BufferAssignment::NewBuffer(b)) => Some(b),
                _ => None,
            }
        });

        if let Some(buffer) = buffer_opt {
            let result = with_buffer_contents(&buffer, |ptr, len, data| {
                let raw = unsafe { std::slice::from_raw_parts(ptr, len) };
                shm_to_rgba(raw, &data)
            });

            match result {
                Ok(Some((rgba, w, h))) => {
                    self.frame_serial = self.frame_serial.wrapping_add(1);
                    let _ = self.frame_tx.send(Frame {
                        rgba, width: w, height: h, serial: self.frame_serial,
                    });
                }
                Ok(None) => tracing::trace!("commit: unsupported buffer format"),
                Err(e)   => tracing::trace!("commit: buffer access error: {e:?}"),
            }

            buffer.release();
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState { &self.shm_state }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg_shell_state }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let size = (self.output_w as i32, self.output_h as i32);
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(size.into());
        });
        surface.send_configure();

        let wl = surface.wl_surface().clone();
        let serial = self.next_serial();
        let kb = self.keyboard.clone();
        kb.set_focus(self, Some(wl), serial);

        self.toplevels.push(surface);
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
    fn reposition_request(&mut self, _s: PopupSurface, _p: PositionerState, _t: u32) {}
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus  = WlSurface;
    type TouchFocus    = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> { &mut self.seat_state }
    fn focus_changed(&mut self, _s: &Seat<Self>, _f: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _s: &Seat<Self>, _i: CursorImageStatus) {}
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl OutputHandler for State {}

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState { &mut self.xdg_activation }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        _data:  XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // Simple policy: always grant — focus the requesting surface for keyboard.
        let serial = self.next_serial();
        let kb = self.keyboard.clone();
        kb.set_focus(self, Some(surface), serial);
    }
}

delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_seat!(State);
delegate_output!(State);
delegate_xdg_activation!(State);

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn shm_to_rgba(raw: &[u8], data: &smithay::wayland::shm::BufferData) -> Option<(Vec<u8>, u32, u32)> {
    let w = data.width  as usize;
    let h = data.height as usize;
    let stride = data.stride as usize;
    if stride < w * 4 || raw.len() < stride * h { return None; }

    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row_start = data.offset as usize + y * stride;
        let row = &raw[row_start .. row_start + w * 4];
        for px in row.chunks_exact(4) {
            let (r, g, b) = match data.format {
                wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => (px[0], px[1], px[2]),
                wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888 => (px[2], px[1], px[0]),
                _ => return None,
            };
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Some((out, w as u32, h as u32))
}

fn send_frame_callbacks(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            for cb in guard.current().frame_callbacks.drain(..) {
                cb.done(time);
            }
        },
        |_, _, &()| true,
    );
}

fn apply_input(state: &mut State, cmd: InputCmd) {
    use smithay::backend::input::{ButtonState as BState, KeyState};
    let serial = state.next_serial();
    let time   = 0u32;

    match cmd {
        InputCmd::Key { keycode, pressed, .. } => {
            let ks = if pressed { KeyState::Pressed } else { KeyState::Released };
            // xkbcommon Keycode is an X11 keycode = evdev + 8.
            let kb = state.keyboard.clone();
            kb.input::<(), _>(
                state, (keycode + 8).into(), ks, serial, time,
                |_, _: &ModifiersState, _: KeysymHandle<'_>| FilterResult::Forward,
            );
        }

        InputCmd::PointerMotionAbs { x, y, width, height } => {
            // Caller works in (width × height) pixel space; rescale to our output.
            let nx = if width  > 0 { x as f64 * state.output_w as f64 / width  as f64 } else { x as f64 };
            let ny = if height > 0 { y as f64 * state.output_h as f64 / height as f64 } else { y as f64 };
            state.pointer_pos = (nx, ny);

            let focus = state.toplevels.first()
                .map(|t| (t.wl_surface().clone(), (0.0_f64, 0.0_f64).into()));
            let ptr = state.pointer.clone();
            ptr.motion(state, focus, &MotionEvent {
                location: (nx, ny).into(), serial, time,
            });
            ptr.frame(state);
        }

        InputCmd::PointerButton { button, pressed } => {
            let bs = if pressed { BState::Pressed } else { BState::Released };
            let ptr = state.pointer.clone();
            ptr.button(state, &ButtonEvent { button, state: bs, serial, time });
            ptr.frame(state);
        }

        InputCmd::Scroll { v120 } => {
            // v120 = 120 per notch (Windows convention). Convert to a 15px-per-notch
            // continuous value as well; clients pick whichever they understand.
            let notches = v120 as f64 / 120.0;
            let mut f = AxisFrame::new(time)
                .source(smithay::backend::input::AxisSource::Wheel);
            f.axis = (0.0, notches * 15.0);
            f.v120 = Some((0, v120));
            let ptr = state.pointer.clone();
            ptr.axis(state, f);
            ptr.frame(state);
        }
    }
}

// ─── Run loop ─────────────────────────────────────────────────────────────────

/// Bundle of state passed through calloop callbacks.
pub struct LoopData {
    pub state:   State,
    pub display: Display<State>,
}

pub fn run(
    socket_name: &str,
    width:  u32,
    height: u32,
    spawn:  Option<Vec<String>>,
    frame_tx: mpsc::Sender<Frame>,
    input_rx: mpsc::Receiver<InputCmd>,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    let display: Display<State> = Display::new()
        .map_err(|e| io::Error::other(format!("display: {e}")))?;
    let dh = display.handle();

    let compositor_state = CompositorState::new::<State>(&dh);
    let shm_state        = ShmState::new::<State>(&dh, vec![]);
    let xdg_shell_state  = XdgShellState::new::<State>(&dh);
    let xdg_activation   = XdgActivationState::new::<State>(&dh);
    let output_manager   = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let mut seat_state   = SeatState::<State>::new();
    let mut seat         = seat_state.new_wl_seat(&dh, "veil-seat");
    let keyboard = seat
        .add_keyboard(XkbConfig::default(), 200, 25)
        .map_err(|e| io::Error::other(format!("keyboard: {e}")))?;
    let pointer = seat.add_pointer();

    let output = Output::new("veil-host-0".into(), PhysicalProperties {
        size: (0, 0).into(),
        subpixel: Subpixel::Unknown,
        make:  "veil".into(),
        model: "host".into(),
    });
    let mode = OutputMode {
        size:    (width as i32, height as i32).into(),
        refresh: 60_000,
    };
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);
    let _output_global = output.create_global::<State>(&dh);

    let state = State {
        compositor_state, xdg_shell_state, shm_state, seat_state,
        xdg_activation, output_manager,
        seat, keyboard, pointer,
        output,
        output_w: width, output_h: height,
        pointer_pos: (0.0, 0.0),
        toplevels: Vec::new(),
        frame_tx,
        serial_counter: 0,
        frame_serial:   0,
        running:        true,
    };

    let mut data = LoopData { state, display };

    // ── Calloop event loop ────────────────────────────────────────────────────
    let mut event_loop: EventLoop<'static, LoopData> = EventLoop::try_new()
        .map_err(|e| io::Error::other(format!("event_loop: {e}")))?;
    let handle = event_loop.handle();

    // 1. Wayland listening socket → accept clients.
    let listener_source = ListeningSocketSource::with_name(socket_name)
        .map_err(|e| io::Error::other(format!("bind {socket_name}: {e}")))?;
    let bound_name = listener_source.socket_name().to_os_string();
    handle.insert_source(listener_source, |stream, _, data| {
        let _ = data.display.handle()
            .insert_client(stream, Arc::new(ClientState::default()));
    }).map_err(|e| io::Error::other(format!("insert listener: {e}")))?;
    tracing::info!("listening on WAYLAND_DISPLAY={:?}", bound_name);

    // 2. Wayland display fd → dispatch protocol messages.
    let wl_fd = data.display.backend().poll_fd().as_raw_fd();
    let wl_src = Generic::new(
        unsafe { FdWrapper::new(wl_fd) }, Interest::READ, CMode::Level,
    );
    handle.insert_source(wl_src, |_, _, data| {
        data.display.dispatch_clients(&mut data.state)
            .map_err(|e| { tracing::error!("dispatch: {e}"); e })?;
        Ok(PostAction::Continue)
    }).map_err(|e| io::Error::other(format!("insert display: {e}")))?;

    // 3. XWayland — spawn it and listen for the Ready event so we can set
    //    DISPLAY for any X11 children we later spawn. Failure is non-fatal:
    //    if Xwayland isn't installed, we just lose X11 compat.
    let xwayland_display: Arc<std::sync::Mutex<Option<u32>>> = Arc::new(std::sync::Mutex::new(None));
    match XWayland::spawn(
        &data.display.handle(),
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_user_data| {},
    ) {
        Ok((xwayland, _x_client)) => {
            let xd = xwayland_display.clone();
            handle.insert_source(xwayland, move |event, _, _data| {
                match event {
                    XWaylandEvent::Ready { x11_socket: _, display_number } => {
                        tracing::info!("XWayland ready on DISPLAY=:{display_number}");
                        *xd.lock().unwrap() = Some(display_number);
                        std::env::set_var("DISPLAY", format!(":{display_number}"));
                    }
                    XWaylandEvent::Error => {
                        tracing::error!("XWayland startup failed");
                    }
                }
            }).map_err(|e| io::Error::other(format!("insert xwayland: {e}")))?;
        }
        Err(e) => tracing::warn!("XWayland unavailable: {e} — X11 apps will not work"),
    }

    // 4. Periodic timer: drain input channel, send frame callbacks,
    //    flush clients, check stop flag. 8 ms tick = ~120 Hz ceiling.
    let start = Instant::now();
    let socket_name_owned = socket_name.to_string();
    let stop_t = stop.clone();
    let tick = Timer::immediate();
    handle.insert_source(tick, move |_, _, data| {
        // Drain input cmds.
        while let Ok(cmd) = input_rx.try_recv() {
            apply_input(&mut data.state, cmd);
        }

        // Send frame callbacks for live toplevels.
        let time = start.elapsed().as_millis() as u32;
        let surfaces: Vec<WlSurface> = data.state.toplevels.iter()
            .filter(|t| t.alive())
            .map(|t| t.wl_surface().clone())
            .collect();
        for s in &surfaces { send_frame_callbacks(s, time); }

        // Flush outgoing wayland messages.
        let _ = data.display.flush_clients();

        if stop_t.load(Ordering::Relaxed) || !data.state.running {
            TimeoutAction::Drop
        } else {
            TimeoutAction::ToDuration(Duration::from_millis(8))
        }
    }).map_err(|e| io::Error::other(format!("insert tick: {e}")))?;

    // 5. Spawn the hosted client after the socket is live.
    if let Some(argv) = spawn {
        if !argv.is_empty() {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            cmd.env("WAYLAND_DISPLAY", &socket_name_owned);
            // Don't unset DISPLAY — XWayland may have set it by the time
            // the child execs, or will shortly. Children that prefer
            // wayland (anything modern) will use WAYLAND_DISPLAY first.
            // For Chromium / Electron, hint at software rendering so
            // they don't immediately die on missing dmabuf.
            cmd.env("LIBGL_ALWAYS_SOFTWARE", "1");
            cmd.env("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
            match cmd.spawn() {
                Ok(_)  => tracing::info!("spawned: {:?}", argv),
                Err(e) => tracing::error!("spawn {:?} failed: {e}", argv),
            }
        }
    }

    // ── Drive the loop ────────────────────────────────────────────────────────
    event_loop.run(Some(Duration::from_millis(16)), &mut data, |data| {
        // Post-dispatch hook: flush again to push anything generated during dispatch.
        let _ = data.display.flush_clients();
        if stop.load(Ordering::Relaxed) || !data.state.running {
            data.state.running = false;
        }
    }).map_err(|e| io::Error::other(format!("run: {e}")))?;

    Ok(())
}
