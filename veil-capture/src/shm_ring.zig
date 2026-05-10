const std = @import("std");

const MAGIC = 0x5645_4C43; // "VELC"

pub const FrameHeader = extern struct {
    magic: u32,
    frame_id: u64,
    width: u32,
    height: u32,
    stride: u32,
    format: u32, // 0=RGBA8888
    timestamp: u64,
};

/// Simple double-buffered frame pool in shared memory.
/// Writer: fills frame A, writes header, increments frame_id
/// Reader: checks frame_id, reads header, copies frame data
pub const FrameRing = struct {
    mmap_ptr: *u8,
    mmap_size: usize,
    frame_size: usize,
    current_frame_id: u64,

    pub fn init(mmap_ptr: *u8, mmap_size: usize, width: u32, height: u32) FrameRing {
        const header_size = @sizeOf(FrameHeader);
        const frame_size = header_size + width * height * 4;
        return .{
            .mmap_ptr = mmap_ptr,
            .mmap_size = mmap_size,
            .frame_size = frame_size,
            .current_frame_id = 0,
        };
    }

    pub fn write_frame(self: *FrameRing, rgba: []const u8, width: u32, height: u32, ts: u64) bool {
        if (rgba.len < width * height * 4) return false;
        if (self.frame_size > self.mmap_size) return false;

        const hdr = @as(*FrameHeader, @ptrCast(@alignCast(self.mmap_ptr)));
        hdr.magic = MAGIC;
        hdr.width = width;
        hdr.height = height;
        hdr.stride = width * 4;
        hdr.format = 0; // RGBA8888
        hdr.timestamp = ts;
        hdr.frame_id = self.current_frame_id +% 1; // wrap-around

        const pixel_start = @as([*]u8, @ptrCast(self.mmap_ptr)) + @sizeOf(FrameHeader);
        @memcpy(pixel_start[0 .. width * height * 4], rgba);

        self.current_frame_id = hdr.frame_id;
        return true;
    }

    pub fn read_frame(self: *FrameRing) ?struct { hdr: FrameHeader, data: []u8 } {
        const hdr = @as(*const FrameHeader, @ptrCast(@alignCast(self.mmap_ptr))).*;
        if (hdr.magic != MAGIC or hdr.frame_id == self.current_frame_id) return null;

        self.current_frame_id = hdr.frame_id;
        const pixel_start = @as([*]u8, @ptrCast(self.mmap_ptr)) + @sizeOf(FrameHeader);
        const pixel_size = hdr.width * hdr.height * 4;
        if (pixel_size > self.mmap_size -| @sizeOf(FrameHeader)) return null;

        return .{
            .hdr = hdr,
            .data = pixel_start[0..pixel_size],
        };
    }
};

/// Create a memfd and return fd + ptr
pub fn create_shm_fd(pid: u32, size: usize) !struct { fd: i32, ptr: *u8 } {
    const path = try std.fmt.allocPrintZ(std.heap.c_allocator, "veil_{}", .{pid});
    defer std.heap.c_allocator.free(path);

    const c = @cImport({
        @cInclude("sys/memfd.h");
        @cInclude("sys/mman.h");
        @cInclude("unistd.h");
    });

    const fd = c.memfd_create(path, c.MFD_CLOEXEC);
    if (fd < 0) return error.MemfdCreateFailed;
    errdefer _ = c.close(fd);

    if (c.ftruncate(fd, @as(c.off_t, @intCast(size))) < 0) {
        return error.FtruncateFailed;
    }

    const ptr = c.mmap(null, size, c.PROT_READ | c.PROT_WRITE, c.MAP_SHARED, fd, 0);
    if (ptr == c.MAP_FAILED) return error.MmapFailed;

    return .{
        .fd = fd,
        .ptr = @as(*u8, @ptrCast(ptr)),
    };
}

pub fn cleanup_shm(ptr: *u8, size: usize) void {
    const c = @cImport({
        @cInclude("sys/mman.h");
    });
    _ = c.munmap(ptr, size);
}
