const std = @import("std");
const shm_ring = @import("shm_ring.zig");
const convert = @import("convert.zig");

// ── C imports ──────────────────────────────────────────────────────────────

const c = @cImport({
    @cInclude("unistd.h");
    @cInclude("sys/mman.h");
    @cInclude("string.h");
    @cInclude("EGL/egl.h");
    @cInclude("stdio.h");
});

// ── Global state (thread-unsafe but acceptable for LD_PRELOAD) ──────────────

var capture_ring: ?*shm_ring.FrameRing = null;
var capture_mmap: ?*u8 = null;
var capture_size: usize = 0;
var last_buffer_data: ?[]u8 = null;
var last_buffer_w: u32 = 0;
var last_buffer_h: u32 = 0;
var frame_counter: u64 = 0;

// Constructor: runs when library is loaded
export fn veil_lib_init() callconv(.C) void {
    const f = c.fopen("/tmp/veil_preload_loaded", "w");
    if (f != null) {
        _ = c.fprintf(f, "veil LD_PRELOAD loaded\n");
        _ = c.fclose(f);
    }
}

// ── Wayland opaque types (we only pass them through) ────────────────────────

pub const WlShm = opaque {};
pub const WlShmPool = opaque {};
pub const WlBuffer = opaque {};
pub const WlSurface = opaque {};

// ── Real Wayland function pointers ─────────────────────────────────────────

var real_wl_shm_pool_create_buffer: ?*const fn (*WlShmPool, i32, i32, i32, i32, u32) callconv(.C) *WlBuffer = null;
var real_wl_surface_attach: ?*const fn (*WlSurface, ?*WlBuffer, i32, i32) callconv(.C) void = null;
var real_wl_surface_commit: ?*const fn (*WlSurface) callconv(.C) void = null;
var real_eglSwapBuffers: ?*const fn (c.EGLDisplay, c.EGLSurface) callconv(.C) c.EGLBoolean = null;

// ── Initialization ─────────────────────────────────────────────────────────

fn init_capture() void {
    if (capture_ring != null) return; // Already initialized

    const pid = c.getpid();
    const capture_w = 1920;
    const capture_h = 1080;
    const frame_size = @sizeOf(shm_ring.FrameHeader) + capture_w * capture_h * 4;

    // Try to create SHM
    const result = shm_ring.create_shm_fd(@as(u32, @intCast(pid)), frame_size) catch {
        return;
    };

    capture_mmap = result.ptr;
    capture_size = frame_size;

    var ring = shm_ring.FrameRing.init(result.ptr, frame_size, capture_w, capture_h);
    // We can't use a global FrameRing* because it needs to live somewhere
    // Instead, store the mmap and reconstruct on reads
}

fn write_frame_to_shm(rgba: [*]const u8, w: u32, h: u32) void {
    if (capture_mmap == null) {
        init_capture();
    }
    if (capture_mmap == null) return;

    var ring = shm_ring.FrameRing.init(capture_mmap.?, capture_size, w, h);
    frame_counter +%= 1;
    _ = ring.write_frame(rgba[0 .. w * h * 4], w, h, frame_counter);
}

// ── Wayland interception ───────────────────────────────────────────────────

/// Hook: wl_shm_pool_create_buffer
/// Track buffer allocation so we can intercept commits
pub export fn wl_shm_pool_create_buffer(
    pool: *WlShmPool,
    offset: i32,
    width: i32,
    height: i32,
    stride: i32,
    format: u32,
) callconv(.C) *WlBuffer {
    // Call the real function
    const real_fn = real_wl_shm_pool_create_buffer orelse {
        // Fallback if we couldn't load the real function
        return @as(*WlBuffer, @ptrFromInt(0));
    };
    const buffer = real_fn(pool, offset, width, height, stride, format);

    // Track dimensions for later
    last_buffer_w = @as(u32, @intCast(width));
    last_buffer_h = @as(u32, @intCast(height));

    return buffer;
}

/// Hook: wl_surface_attach
pub export fn wl_surface_attach(
    surface: *WlSurface,
    buffer: ?*WlBuffer,
    x: i32,
    y: i32,
) callconv(.C) void {
    const real_fn = real_wl_surface_attach orelse return;
    real_fn(surface, buffer, x, y);
}

/// Hook: wl_surface_commit
/// When the surface commits, capture its buffer
pub export fn wl_surface_commit(surface: *WlSurface) callconv(.C) void {
    const real_fn = real_wl_surface_commit orelse return;

    // Debug: write to a marker file if this is called
    if (true) {
        const f = c.fopen("/tmp/veil_hook_called", "a");
        if (f != null) {
            _ = c.fprintf(f, "commit %dx%d\n", last_buffer_w, last_buffer_h);
            _ = c.fclose(f);
        }
    }

    // Call the real commit (this makes the buffer visible)
    real_fn(surface);

    // Try to capture after commit
    if (last_buffer_w > 0 and last_buffer_h > 0) {
        init_capture();
    }
}

/// Hook: eglSwapBuffers
/// Capture after EGL buffer swap
pub export fn eglSwapBuffers(
    display: c.EGLDisplay,
    surface: c.EGLSurface,
) callconv(.C) c.EGLBoolean {
    const real_fn = real_eglSwapBuffers orelse {
        return 0; // EGL_FALSE
    };

    const result = real_fn(display, surface);

    // After swap, the buffer is ready to read
    // In a full implementation, we'd read the framebuffer here
    init_capture();

    return result;
}

// ── Rust FFI utilities ─────────────────────────────────────────────────────

pub export fn veil_xrgb_to_rgba(
    src: [*]const u8,
    src_len: usize,
    width: u32,
    height: u32,
    out: [*]u8,
) bool {
    if (src_len < width * height * 4) return false;
    const src_slice = src[0..src_len];
    for (0..width * height) |i| {
        const s_off = i * 4;
        const d_off = i * 4;
        out[d_off + 0] = src_slice[s_off + 2]; // R
        out[d_off + 1] = src_slice[s_off + 1]; // G
        out[d_off + 2] = src_slice[s_off + 0]; // B
        out[d_off + 3] = 0xFF; // A
    }
    return true;
}

pub export fn veil_crop_rgba(
    src: [*]const u8,
    src_len: usize,
    src_w: u32,
    src_h: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    scale: i32,
    out: [*]u8,
) u32 {
    const s = @max(1, scale);
    const px = @max(0, x) * s;
    const py = @max(0, y) * s;
    const pw = w * @as(u32, @intCast(s));
    const ph = h * @as(u32, @intCast(s));
    const cw = @min(pw, src_w -| @as(u32, @intCast(px)));
    const ch = @min(ph, src_h -| @as(u32, @intCast(py)));

    if (cw == 0 or ch == 0) return 0;

    var row = py;
    while (row < py + ch) : (row += 1) {
        const s_off = (row * src_w + @as(u32, @intCast(px))) * 4;
        const d_off = (row - py) * cw * 4;
        const len = cw * 4;
        if (s_off + len <= src_len) {
            @memcpy(out[d_off .. d_off + len], src[s_off .. s_off + len]);
        }
    }
    return cw * ch * 4;
}

pub export fn veil_shm_ring_init(
    mmap_ptr: *u8,
    mmap_size: usize,
    width: u32,
    height: u32,
) ?*shm_ring.FrameRing {
    capture_mmap = mmap_ptr;
    capture_size = mmap_size;
    return @as(*shm_ring.FrameRing, @ptrFromInt(@intFromPtr(mmap_ptr)));
}

pub export fn veil_shm_ring_write(
    _: *shm_ring.FrameRing,
    rgba: [*]const u8,
    len: usize,
    width: u32,
    height: u32,
    ts: u64,
) bool {
    if (capture_mmap == null) return false;
    var ring = shm_ring.FrameRing.init(capture_mmap.?, capture_size, width, height);
    return ring.write_frame(rgba[0..len], width, height, ts);
}

pub export fn veil_shm_ring_read(
    _: *shm_ring.FrameRing,
    out_hdr: *shm_ring.FrameHeader,
    out_data: *[*]u8,
) u32 {
    if (capture_mmap == null) return 0;
    var ring = shm_ring.FrameRing.init(capture_mmap.?, capture_size, 1920, 1080);
    if (ring.read_frame()) |frame| {
        out_hdr.* = frame.hdr;
        out_data.* = frame.data.ptr;
        return @as(u32, @intCast(frame.data.len));
    }
    return 0;
}

// ── Exports for LD_PRELOAD ─────────────────────────────────────────────────

// Export the hook functions so they override Wayland/EGL symbols
// The linker will use these instead of the real library functions
