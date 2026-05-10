/// Synchronous wlr-screencopy client.
///
/// Connects to the compositor's Wayland socket, binds
/// `zwlr_screencopy_manager_v1`, `wl_shm`, and `wl_output`, then exposes a
/// blocking `capture_region()` call.  Designed to run on a dedicated background
/// thread — the event loop blocks inside each call until the frame is ready.
///
/// Protocol flow (per frame):
///   1. `manager.capture_output_region(output, x, y, w, h)` → frame proxy
///   2. Compositor fires `buffer(Xrgb8888, w, h, stride)` → allocate SHM,
///      create wl_shm_pool + wl_buffer, call `frame.copy(buffer)`
///   3. Compositor fires `ready` → pixels in mmap; return to caller
///   4. Compositor fires `failed` → return None
use std::os::unix::net::UnixStream;
use std::ptr;
use std::slice;

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{memfd_create, ftruncate, MemfdFlags};
use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};

use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_output::{self, WlOutput},
        wl_registry::{self, WlRegistry},
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};

/* ── SHM allocation ──────────────────────────────────────────────────────── */

struct ShmAlloc {
    fd:  OwnedFd,
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for ShmAlloc {}
unsafe impl Sync for ShmAlloc {}

impl ShmAlloc {
    fn new(len: usize) -> Option<Self> {
        if len == 0 { return None; }
        let fd = memfd_create("veil-screencopy", MemfdFlags::CLOEXEC).ok()?;
        ftruncate(&fd, len as u64).ok()?;
        let ptr = unsafe {
            mmap(ptr::null_mut(), len, ProtFlags::READ | ProtFlags::WRITE,
                 MapFlags::SHARED, &fd, 0).ok()?
        } as *mut u8;
        Some(Self { fd, ptr, len })
    }

    fn data(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for ShmAlloc {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            let _ = unsafe { munmap(self.ptr.cast(), self.len) };
        }
    }
}

/* ── Per-frame pending state ─────────────────────────────────────────────── */

struct PendingFrame {
    alloc:  ShmAlloc,
    pool:   WlShmPool,
    wl_buf: WlBuffer,
    width:  u32,
    height: u32,
    stride: u32,
}

/* ── Client state ────────────────────────────────────────────────────────── */

struct CaptureState {
    shm:          Option<WlShm>,
    manager:      Option<ZwlrScreencopyManagerV1>,
    outputs:      Vec<WlOutput>,
    output_scale: i32,
    target_idx:   usize,
    // per-frame
    pending:      Option<PendingFrame>,
    frame_ready:  bool,
    frame_failed: bool,
}

impl CaptureState {
    fn new(target_idx: usize) -> Self {
        Self {
            shm: None, manager: None, outputs: Vec::new(),
            output_scale: 1, target_idx,
            pending: None, frame_ready: false, frame_failed: false,
        }
    }
}

/* ── Registry ────────────────────────────────────────────────────────────── */

impl Dispatch<WlRegistry, ()> for CaptureState {
    fn event(state: &mut Self, reg: &WlRegistry, ev: wl_registry::Event,
             _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        let wl_registry::Event::Global { name, interface, version } = ev else { return };
        match interface.as_str() {
            "wl_shm" =>
                state.shm = Some(reg.bind(name, 1, qh, ())),
            "zwlr_screencopy_manager_v1" =>
                state.manager = Some(reg.bind(name, version.min(3), qh, ())),
            "wl_output" =>
                state.outputs.push(reg.bind(name, version.clamp(2, 4), qh, ())),
            _ => {}
        }
    }
}

/* ── Frame events — the actual work ─────────────────────────────────────── */

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(state: &mut Self, frame: &ZwlrScreencopyFrameV1, ev: zwlr_screencopy_frame_v1::Event,
             _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        use zwlr_screencopy_frame_v1::Event;
        match ev {
            Event::Buffer { format, width, height, stride } => {
                if format != WEnum::Value(wl_shm::Format::Xrgb8888) { return; }
                if state.pending.is_some() { return; }

                let Some(shm) = &state.shm else { return };
                let len = (stride * height) as usize;
                let Some(alloc) = ShmAlloc::new(len) else { return };

                let pool   = shm.create_pool(alloc.fd.as_fd(), len as i32, qh, ());
                let wl_buf = pool.create_buffer(
                    0, width as i32, height as i32, stride as i32,
                    wl_shm::Format::Xrgb8888, qh, (),
                );
                frame.copy(&wl_buf);

                state.pending = Some(PendingFrame { alloc, pool, wl_buf, width, height, stride });
            }
            Event::Ready { .. } => {
                state.frame_ready = true;
            }
            Event::Failed => {
                state.frame_failed = true;
                if let Some(pf) = state.pending.take() {
                    pf.wl_buf.destroy();
                    pf.pool.destroy();
                }
            }
            _ => {}
        }
    }
}

/* ── Scale event from wl_output ─────────────────────────────────────────── */

impl Dispatch<WlOutput, ()> for CaptureState {
    fn event(state: &mut Self, _: &WlOutput, ev: wl_output::Event,
             _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Scale { factor } = ev {
            state.output_scale = factor;
        }
    }
}

/* ── noop impls for objects we don't care about ──────────────────────────── */

delegate_noop!(CaptureState: ignore WlShm);
delegate_noop!(CaptureState: ignore WlShmPool);
delegate_noop!(CaptureState: ignore WlBuffer);
delegate_noop!(CaptureState: ignore ZwlrScreencopyManagerV1);

/* ── Public API ──────────────────────────────────────────────────────────── */

pub struct WaylandCapture {
    conn:  Connection,
    eq:    wayland_client::EventQueue<CaptureState>,
    state: CaptureState,
    qh:    QueueHandle<CaptureState>,
}

impl WaylandCapture {
    /// Connect and capture from a specific output index (0 = primary, 1 = secondary, etc).
    /// Falls back to 0 if the requested index doesn't exist.
    pub fn connect_output(output_index: usize) -> Option<Self> {
        let conn = Connection::connect_to_env().ok()?;
        let mut eq = conn.new_event_queue();
        let qh     = eq.handle();

        let display = conn.display();
        display.get_registry(&qh, ());

        let mut state = CaptureState::new(output_index);
        eq.roundtrip(&mut state).ok()?;
        eq.roundtrip(&mut state).ok()?;

        if state.manager.is_none() {
            eprintln!("[wayland_capture] zwlr_screencopy_manager_v1 not available");
            return None;
        }
        if state.outputs.is_empty() {
            eprintln!("[wayland_capture] no wl_output bound");
            return None;
        }

        // Clamp index to available outputs
        if output_index >= state.outputs.len() {
            eprintln!("[wayland_capture] output {} not available, using 0 (have {})",
                output_index, state.outputs.len());
            state.target_idx = 0;
        }

        eprintln!("[wayland_capture] connected output={}/{} scale={}",
            state.target_idx, state.outputs.len(), state.output_scale);
        Some(Self { conn, eq, state, qh })
    }

    /// Connect to primary output (convenience wrapper).
    pub fn connect() -> Option<Self> {
        Self::connect_output(0)
    }

    /// Connect to a specific Wayland socket by path (e.g. cage's isolated display).
    pub fn connect_to_socket(socket_path: &str) -> Option<Self> {
        let stream = UnixStream::connect(socket_path).ok()?;
        let conn = Connection::from_socket(stream).ok()?;
        let mut eq = conn.new_event_queue();
        let qh = eq.handle();

        conn.display().get_registry(&qh, ());

        let mut state = CaptureState::new(0);
        eq.roundtrip(&mut state).ok()?;
        eq.roundtrip(&mut state).ok()?;

        if state.manager.is_none() {
            eprintln!("[wayland_capture] socket {}: no zwlr_screencopy_manager_v1", socket_path);
            return None;
        }
        if state.outputs.is_empty() {
            eprintln!("[wayland_capture] socket {}: no wl_output", socket_path);
            return None;
        }

        eprintln!("[wayland_capture] connected to {} (scale={})", socket_path, state.output_scale);
        Some(Self { conn, eq, state, qh })
    }

    pub fn output_scale(&self) -> i32 {
        self.state.output_scale.max(1)
    }

    /// Capture a logical-coordinate region of the primary output.
    ///
    /// `x`, `y`, `w`, `h` are in compositor logical pixels (Niri IPC units).
    /// Internally multiplied by output scale to get physical pixels.
    ///
    /// Returns `(width, height, rgba_data)` on success where rgba_data is
    /// RGBA8888 (converted from XRGB8888 LE storage).
    pub fn capture_region(&mut self, x: i32, y: i32, w: u32, h: u32) -> Option<(u32, u32, Vec<u8>)> {
        let scale   = self.state.output_scale.max(1);
        let manager = self.state.manager.as_ref()?;
        let output  = self.state.outputs.get(self.state.target_idx)?;

        let _frame = manager.capture_output_region(
            0, output,
            x * scale, y * scale,
            (w as i32) * scale, (h as i32) * scale,
            &self.qh, (),
        );

        self.state.frame_ready  = false;
        self.state.frame_failed = false;

        loop {
            if self.state.frame_ready || self.state.frame_failed { break; }
            if self.eq.blocking_dispatch(&mut self.state).is_err() {
                return None;
            }
        }

        if self.state.frame_failed { return None; }

        let pf = self.state.pending.take()?;
        let raw = pf.alloc.data().to_vec();
        pf.wl_buf.destroy();
        pf.pool.destroy();

        // XRGB8888 LE: bytes are [B, G, R, X] → convert to [R, G, B, 255]
        let rgba = xrgb_to_rgba(&raw);
        Some((pf.width, pf.height, rgba))
    }

    /// Capture the full target output.
    pub fn capture_full(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let manager = self.state.manager.as_ref()?;
        let output  = self.state.outputs.get(self.state.target_idx)?;

        let _frame = manager.capture_output(0, output, &self.qh, ());

        self.state.frame_ready  = false;
        self.state.frame_failed = false;

        loop {
            if self.state.frame_ready || self.state.frame_failed { break; }
            if self.eq.blocking_dispatch(&mut self.state).is_err() {
                return None;
            }
        }

        if self.state.frame_failed { return None; }

        let pf = self.state.pending.take()?;
        let raw = pf.alloc.data().to_vec();
        pf.wl_buf.destroy();
        pf.pool.destroy();

        let rgba = xrgb_to_rgba(&raw);
        Some((pf.width, pf.height, rgba))
    }
}

fn xrgb_to_rgba(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for chunk in src.chunks_exact(4) {
        out.push(chunk[2]); // R
        out.push(chunk[1]); // G
        out.push(chunk[0]); // B
        out.push(0xFF);     // A
    }
    out
}
