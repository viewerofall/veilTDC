/// Wayland virtual input injection into cage's compositor socket.
///
/// Connects a second Wayland client directly to cage's socket and creates
/// zwp_virtual_keyboard_v1 + zwlr_virtual_pointer_v1 devices. All input is
/// injected at the protocol level — no portal, no xdotool, no uinput daemon.
use std::{
    os::unix::net::UnixStream,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fd::AsFd,
    fs::{memfd_create, ftruncate, MemfdFlags},
    mm::{mmap, munmap, MapFlags, ProtFlags},
};
use wayland_client::{
    delegate_noop,
    protocol::{
        wl_keyboard::{self, WlKeyboard},
        wl_pointer,
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

/* ── Commands ────────────────────────────────────────────────────────────── */

pub enum InputCmd {
    Key { keycode: u32, mods: u32, pressed: bool },
    PointerButton { button: u32, pressed: bool },
    PointerMotionAbs { x: u32, y: u32, width: u32, height: u32 },
    /// Vertical scroll: positive = down, negative = up. One notch = 1 step.
    PointerAxisScroll { steps: i32 },
}

/* ── Keymap reader ───────────────────────────────────────────────────────── */

struct KeymapReader {
    seat:     Option<WlSeat>,
    keyboard: Option<WlKeyboard>,
    keymap:   Option<String>,
}

impl Dispatch<WlRegistry, ()> for KeymapReader {
    fn event(state: &mut Self, reg: &WlRegistry, ev: wl_registry::Event,
             _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            if interface == "wl_seat" && state.seat.is_none() {
                state.seat = Some(reg.bind(name, version.min(5), qh, ()));
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for KeymapReader {
    fn event(state: &mut Self, seat: &WlSeat, ev: wl_seat::Event,
             _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = ev {
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for KeymapReader {
    fn event(state: &mut Self, _: &WlKeyboard, ev: wl_keyboard::Event,
             _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_keyboard::Event::Keymap {
            format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
            fd,
            size,
        } = ev {
            use std::io::Read;
            let mut file: std::fs::File = fd.into();
            let mut s = String::with_capacity(size as usize);
            let _ = file.read_to_string(&mut s);
            if !s.is_empty() {
                state.keymap = Some(s);
            }
        }
    }
}

/// Connects to the host compositor (WAYLAND_DISPLAY) and reads the XKB keymap
/// from the seat's keyboard. Returns the keymap as text.
pub fn get_host_keymap() -> Option<String> {
    let conn = Connection::connect_to_env().ok()?;
    let mut eq = conn.new_event_queue::<KeymapReader>();
    let qh    = eq.handle();

    let mut state = KeymapReader { seat: None, keyboard: None, keymap: None };
    conn.display().get_registry(&qh, ());

    // Pass 1: get globals → seat
    eq.roundtrip(&mut state).ok()?;
    // Pass 2: seat capabilities → keyboard
    eq.roundtrip(&mut state).ok()?;
    // Pass 3: keyboard keymap event
    eq.roundtrip(&mut state).ok()?;

    state.keymap
}

/* ── Virtual input state ─────────────────────────────────────────────────── */

struct VInputState {
    seat:       Option<WlSeat>,
    vk_manager: Option<ZwpVirtualKeyboardManagerV1>,
    vp_manager: Option<ZwlrVirtualPointerManagerV1>,
}

impl Dispatch<WlRegistry, ()> for VInputState {
    fn event(state: &mut Self, reg: &WlRegistry, ev: wl_registry::Event,
             _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        let wl_registry::Event::Global { name, interface, version } = ev else { return };
        match interface.as_str() {
            "wl_seat" if state.seat.is_none() =>
                state.seat = Some(reg.bind(name, version.min(5), qh, ())),
            "zwp_virtual_keyboard_manager_v1" =>
                state.vk_manager = Some(reg.bind(name, 1, qh, ())),
            "zwlr_virtual_pointer_manager_v1" =>
                state.vp_manager = Some(reg.bind(name, version.min(2), qh, ())),
            _ => {}
        }
    }
}

delegate_noop!(VInputState: ignore WlSeat);
delegate_noop!(VInputState: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(VInputState: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(VInputState: ignore ZwpVirtualKeyboardV1);
delegate_noop!(VInputState: ignore ZwlrVirtualPointerV1);

/* ── Keymap memfd ────────────────────────────────────────────────────────── */

struct KeymapFd {
    fd:  rustix::fd::OwnedFd,
    len: usize,
    ptr: *mut u8,
}

unsafe impl Send for KeymapFd {}

impl KeymapFd {
    fn new(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        // Include null terminator — compositor expects it
        let len = bytes.len() + 1;
        let fd  = memfd_create("veil-keymap", MemfdFlags::CLOEXEC).ok()?;
        ftruncate(&fd, len as u64).ok()?;
        let ptr = unsafe {
            mmap(std::ptr::null_mut(), len, ProtFlags::READ | ProtFlags::WRITE,
                 MapFlags::SHARED, &fd, 0).ok()?
        } as *mut u8;
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            *ptr.add(bytes.len()) = 0; // null terminator
        }
        Some(Self { fd, len, ptr })
    }
}

impl Drop for KeymapFd {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            let _ = unsafe { munmap(self.ptr.cast(), self.len) };
        }
    }
}

/* ── Public API ──────────────────────────────────────────────────────────── */

pub struct WaylandInput {
    tx: mpsc::SyncSender<InputCmd>,
}

impl WaylandInput {
    pub fn connect_to_socket(socket_path: &str) -> Option<Self> {
        let keymap = get_host_keymap()?;

        let stream = UnixStream::connect(socket_path).ok()?;
        let conn   = Connection::from_socket(stream).ok()?;
        let mut eq = conn.new_event_queue::<VInputState>();
        let qh     = eq.handle();

        let mut state = VInputState { seat: None, vk_manager: None, vp_manager: None };
        conn.display().get_registry(&qh, ());
        eq.roundtrip(&mut state).ok()?;

        let seat       = state.seat.take()?;
        let vk_manager = state.vk_manager.take()?;
        let vp_manager = state.vp_manager.take()?;

        let keyboard = vk_manager.create_virtual_keyboard(&seat, &qh, ());
        let pointer  = vp_manager.create_virtual_pointer(Some(&seat), &qh, ());

        // Push the keymap to the virtual keyboard (XKB_V1 = 1)
        let km = KeymapFd::new(&keymap)?;
        keyboard.keymap(1, km.fd.as_fd(), km.len as u32);
        conn.flush().ok()?;

        let (tx, rx) = mpsc::sync_channel::<InputCmd>(64);

        thread::spawn(move || {
            // Keep keymap fd alive for the thread lifetime
            let _km = km;
            let start = Instant::now();
            let mut mods: u32 = 0;

            loop {
                if eq.dispatch_pending(&mut state).is_err() { break; }

                match rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(InputCmd::Key { keycode, mods: new_mods, pressed }) => {
                        let t = start.elapsed().as_millis() as u32;
                        if new_mods != mods {
                            keyboard.modifiers(new_mods, 0, 0, 0);
                            mods = new_mods;
                        }
                        keyboard.key(t, keycode, if pressed { 1 } else { 0 });
                        // Reset modifiers after key release
                        if !pressed && mods != 0 {
                            keyboard.modifiers(0, 0, 0, 0);
                            mods = 0;
                        }
                        conn.flush().ok();
                    }
                    Ok(InputCmd::PointerButton { button, pressed }) => {
                        let t     = start.elapsed().as_millis() as u32;
                        let state = if pressed {
                            wl_pointer::ButtonState::Pressed
                        } else {
                            wl_pointer::ButtonState::Released
                        };
                        pointer.button(t, button, state);
                        pointer.frame();
                        conn.flush().ok();
                    }
                    Ok(InputCmd::PointerMotionAbs { x, y, width, height }) => {
                        let t = start.elapsed().as_millis() as u32;
                        pointer.motion_absolute(t, x, y, width, height);
                        pointer.frame();
                        conn.flush().ok();
                    }
                    Ok(InputCmd::PointerAxisScroll { steps }) => {
                        use wayland_client::protocol::wl_pointer::Axis;
                        let t = start.elapsed().as_millis() as u32;
                        // 15.0 per step matches most compositor scroll sensitivity defaults
                        pointer.axis(t, Axis::VerticalScroll, steps as f64 * 15.0);
                        pointer.frame();
                        conn.flush().ok();
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout)      => {}
                }
            }
        });

        Some(Self { tx })
    }

    pub fn send(&self, cmd: InputCmd) {
        let _ = self.tx.try_send(cmd);
    }
}

/* ── Linux evdev keycode constants ───────────────────────────────────────── */

/// Map an ASCII char (lowercase) or special char to a Linux evdev keycode.
/// Returns None for unmappable characters.
pub fn evdev_keycode(ch: char) -> Option<u32> {
    Some(match ch {
        'a'  => 30, 'b'  => 48, 'c'  => 46, 'd'  => 32, 'e'  => 18,
        'f'  => 33, 'g'  => 34, 'h'  => 35, 'i'  => 23, 'j'  => 36,
        'k'  => 37, 'l'  => 38, 'm'  => 50, 'n'  => 49, 'o'  => 24,
        'p'  => 25, 'q'  => 16, 'r'  => 19, 's'  => 31, 't'  => 20,
        'u'  => 22, 'v'  => 47, 'w'  => 17, 'x'  => 45, 'y'  => 21,
        'z'  => 44,
        '1'  => 2,  '2'  => 3,  '3'  => 4,  '4'  => 5,  '5'  => 6,
        '6'  => 7,  '7'  => 8,  '8'  => 9,  '9'  => 10, '0'  => 11,
        '-'  => 12, '='  => 13, '['  => 26, ']'  => 27, '\\'  => 43,
        ';'  => 39, '\'' => 40, '`'  => 41, ','  => 51, '.'  => 52,
        '/'  => 53, ' '  => 57,
        _    => return None,
    })
}



/// Named key codes for use by the CLI's crossterm → InputCmd converter.
pub mod keycodes {
    pub const BACKSPACE: u32 = 14;
    pub const TAB:       u32 = 15;
    pub const ENTER:     u32 = 28;
    pub const ESC:       u32 = 1;
    pub const DELETE:    u32 = 111;
    pub const INSERT:    u32 = 110;
    pub const HOME:      u32 = 102;
    pub const END:       u32 = 107;
    pub const PAGE_UP:   u32 = 104;
    pub const PAGE_DOWN: u32 = 109;
    pub const LEFT:      u32 = 105;
    pub const RIGHT:     u32 = 106;
    pub const UP:        u32 = 103;
    pub const DOWN:      u32 = 108;
    pub const F: [u32; 13] = [0, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88];
}

/// XKB modifier bits (standard evdev+us keymap).
pub mod xkb_mod {
    pub const SHIFT:   u32 = 1;
    pub const CONTROL: u32 = 4;
    pub const ALT:     u32 = 8;
    pub const SUPER:   u32 = 64;
}

/// Linux BTN_ codes for pointer buttons.
pub mod btn {
    pub const LEFT:   u32 = 272;
    pub const RIGHT:  u32 = 273;
    pub const MIDDLE: u32 = 274;
}
