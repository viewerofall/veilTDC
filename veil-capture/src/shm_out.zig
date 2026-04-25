const std = @import("std");
const c = @cImport({
    @cInclude("fcntl.h");
    @cInclude("sys/mman.h");
    @cInclude("unistd.h");
    @cInclude("string.h");
});

pub const CaptureHeader = extern struct {
    magic: u32,
    frame_id: u64,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
};

const MAGIC = 0x5645_4C43;

var gpa = std.heap.GeneralPurposeAllocator(.{}){};
var alloc: std.mem.Allocator = undefined;
var shm_fd: i32 = -1;
var shm_ptr: ?[*]u8 = null;
var shm_size: usize = 0;
var frame_id: u64 = 0;

pub fn init(pid: u32) !void {
    alloc = gpa.allocator();
    var buf: [64]u8 = undefined;
    const name = try std.fmt.bufPrint(&buf, "/veil_{d}", .{pid});
    _ = c.shm_unlink(@ptrCast(name));
    shm_fd = c.shm_open(@ptrCast(name), c.O_CREAT | c.O_RDWR, 0o600);
    if (shm_fd < 0) return error.ShmOpenFailed;
    const initial_size = 256 * 1024;
    if (c.ftruncate(shm_fd, initial_size) < 0) return error.ShmTruncateFailed;
    shm_ptr = @ptrCast(c.mmap(null, initial_size, c.PROT_READ | c.PROT_WRITE, c.MAP_SHARED, shm_fd, 0));
    if (shm_ptr == null or shm_ptr.? == @as([*]u8, @ptrFromInt(@as(usize, @bitCast(@as(isize, -1)))))) return error.ShmMmapFailed;
    shm_size = initial_size;
}

pub fn deinit() void {
    if (shm_ptr != null) {
        _ = c.munmap(@ptrCast(shm_ptr), shm_size);
    }
    if (shm_fd >= 0) {
        _ = c.close(shm_fd);
    }
}

pub fn write_frame(width: u32, height: u32, stride: u32, format: u32, pixels: [*]const u8) void {
    if (shm_ptr == null) return;
    const header_size = @sizeOf(CaptureHeader);
    const pixel_size = height * stride;
    const total_size = header_size + pixel_size;
    if (total_size > shm_size) {
        const new_size = ((total_size + 65535) / 65536) * 65536;
        if (c.ftruncate(shm_fd, @intCast(new_size)) < 0) return;
        _ = c.munmap(@ptrCast(shm_ptr), shm_size);
        shm_ptr = @ptrCast(c.mmap(null, new_size, c.PROT_READ | c.PROT_WRITE, c.MAP_SHARED, shm_fd, 0));
        if (shm_ptr == null) return;
        shm_size = new_size;
    }
    frame_id +%= 1;
    const header: CaptureHeader = .{
        .magic = MAGIC,
        .frame_id = frame_id,
        .width = width,
        .height = height,
        .stride = stride,
        .format = format,
    };
    @memcpy(shm_ptr.?[0..header_size], std.mem.asBytes(&header));
    @memcpy(shm_ptr.?[header_size .. header_size + pixel_size], pixels[0..pixel_size]);
}
