const std = @import("std");

/// Convert XRGB8888 LE (wl_shm format) to RGBA8888.
/// Input bytes: [B, G, R, X] → Output: [R, G, B, A]
pub fn xrgb_to_rgba(allocator: std.mem.Allocator, src: []const u8, width: u32, height: u32) ![]u8 {
    const pixel_count = width * height;
    const out = try allocator.alloc(u8, pixel_count * 4);
    errdefer allocator.free(out);

    for (0..pixel_count) |i| {
        const s_off = i * 4;
        const d_off = i * 4;
        out[d_off + 0] = src[s_off + 2]; // R
        out[d_off + 1] = src[s_off + 1]; // G
        out[d_off + 2] = src[s_off + 0]; // B
        out[d_off + 3] = 0xFF;           // A
    }
    return out;
}

/// Simple nearest-neighbor downscale RGBA to smaller dimensions.
pub fn scale_down(allocator: std.mem.Allocator, src: []const u8, src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) ![]u8 {
    const out = try allocator.alloc(u8, dst_w * dst_h * 4);
    errdefer allocator.free(out);

    var dy: u32 = 0;
    while (dy < dst_h) : (dy += 1) {
        var dx: u32 = 0;
        while (dx < dst_w) : (dx += 1) {
            const sy = (dy * src_h) / dst_h;
            const sx = (dx * src_w) / dst_w;
            const src_off = (sy * src_w + sx) * 4;
            const dst_off = (dy * dst_w + dx) * 4;
            @memcpy(out[dst_off .. dst_off + 4], src[src_off .. src_off + 4]);
        }
    }
    return out;
}

/// Crop RGBA to a region (logical px * scale = physical px).
pub fn crop_rgba(allocator: std.mem.Allocator, src: []const u8, src_w: u32, src_h: u32,
                 x: i32, y: i32, w: u32, h: u32, scale: i32) ![]u8 {
    const s = @max(1, scale);
    const px = @max(0, x) * s;
    const py = @max(0, y) * s;
    const pw = (w * @as(u32, @intCast(s)));
    const ph = (h * @as(u32, @intCast(s)));
    const cw = @min(pw, src_w -| @as(u32, @intCast(px)));
    const ch = @min(ph, src_h -| @as(u32, @intCast(py)));

    if (cw == 0 or ch == 0) return &[_]u8{};

    const out = try allocator.alloc(u8, cw * ch * 4);
    errdefer allocator.free(out);

    var row = py;
    while (row < py + ch) : (row += 1) {
        const s_off = (row * src_w + @as(u32, @intCast(px))) * 4;
        const d_off = (row - py) * cw * 4;
        const len = cw * 4;
        if (s_off + len <= src.len) {
            @memcpy(out[d_off .. d_off + len], src[s_off .. s_off + len]);
        }
    }
    return out;
}
