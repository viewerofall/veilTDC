const std = @import("std");
const convert = @import("convert.zig");
const shm_ring = @import("shm_ring.zig");

// ── Frame conversion utilities (called from Rust) ────────────────────────────

pub export fn veil_xrgb_to_rgba(src: [*]const u8, src_len: usize, width: u32, height: u32, out: [*]u8) bool {
    if (src_len < width * height * 4) return false;
    const src_slice = src[0..src_len];
    for (0..width * height) |i| {
        const s_off = i * 4;
        const d_off = i * 4;
        out[d_off + 0] = src_slice[s_off + 2]; // R
        out[d_off + 1] = src_slice[s_off + 1]; // G
        out[d_off + 2] = src_slice[s_off + 0]; // B
        out[d_off + 3] = 0xFF;                 // A
    }
    return true;
}

pub export fn veil_crop_rgba(src: [*]const u8, src_len: usize, src_w: u32, src_h: u32,
                            x: i32, y: i32, w: u32, h: u32, scale: i32,
                            out: [*]u8) u32 {
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

// ── SHM ring buffer for frame IPC ──────────────────────────────────────────

var ring_instance: shm_ring.FrameRing = undefined;

pub export fn veil_shm_ring_init(mmap_ptr: *u8, mmap_size: usize, width: u32, height: u32) ?*shm_ring.FrameRing {
    ring_instance = shm_ring.FrameRing.init(mmap_ptr, mmap_size, width, height);
    return &ring_instance;
}

pub export fn veil_shm_ring_write(ring: *shm_ring.FrameRing, rgba: [*]const u8, len: usize, width: u32, height: u32, ts: u64) bool {
    return ring.write_frame(rgba[0..len], width, height, ts);
}

pub export fn veil_shm_ring_read(ring: *shm_ring.FrameRing, out_hdr: *shm_ring.FrameHeader, out_data: *[*]u8) u32 {
    if (ring.read_frame()) |frame| {
        out_hdr.* = frame.hdr;
        out_data.* = frame.data.ptr;
        return @as(u32, @intCast(frame.data.len));
    }
    return 0;
}
