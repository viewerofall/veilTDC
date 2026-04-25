const std = @import("std");
const shm_out = @import("shm_out.zig");
const c = @cImport({
    @cInclude("wayland-client.h");
    @cInclude("dlfcn.h");
    @cInclude("stdlib.h");
    @cInclude("string.h");
    @cInclude("unistd.h");
    @cInclude("pthread.h");
});

pub const WlBuffer = opaque {};
pub const WlSurface = opaque {};
pub const WlShmPool = opaque {};
pub const WlShm = opaque {};

const BufferInfo = struct {
    pool_data: [*]u8,
    offset: u32,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
};

const PoolInfo = struct {
    data: [*]u8,
    size: usize,
};

var gpa = std.heap.GeneralPurposeAllocator(.{}){};
var alloc: std.mem.Allocator = undefined;
var buffer_map: std.AutoHashMap(*WlBuffer, BufferInfo) = undefined;
var pool_map: std.AutoHashMap(*WlShmPool, PoolInfo) = undefined;
var surface_map: std.AutoHashMap(*WlSurface, *WlBuffer) = undefined;
var state_mutex: c.pthread_mutex_t = undefined;
var initialized: bool = false;

fn ensure_init() void {
    if (initialized) return;
    initialized = true;
    alloc = gpa.allocator();
    buffer_map = std.AutoHashMap(*WlBuffer, BufferInfo).init(alloc);
    pool_map = std.AutoHashMap(*WlShmPool, PoolInfo).init(alloc);
    surface_map = std.AutoHashMap(*WlSurface, *WlBuffer).init(alloc);
    _ = c.pthread_mutex_init(&state_mutex, null);
    const pid = c.getpid();
    shm_out.init(@intCast(pid)) catch {};
}

fn get_real_fn(comptime name: []const u8) ?*const anyopaque {
    return c.dlsym(c.RTLD_NEXT, @ptrCast(name));
}

pub fn wl_shm_create_pool(shm: *WlShm, fd: i32, size: i32) *WlShmPool {
    ensure_init();
    const real_fn = @as(*const fn (*WlShm, i32, i32)  *WlShmPool, @ptrCast(get_real_fn("wl_shm_create_pool") orelse @panic("dlsym failed")));
    const pool = real_fn(shm, fd, size);
    const c_import = @cImport({
        @cInclude("sys/mman.h");
    });
    const pool_data = @as([*]u8, @ptrCast(c_import.mmap(null, @intCast(size), c_import.PROT_READ | c_import.PROT_WRITE, c_import.MAP_SHARED, fd, 0)));
    if (pool_data != @as([*]u8, @ptrFromInt(@as(usize, @bitCast(@as(isize, -1)))))) {
        _ = c.pthread_mutex_lock(&state_mutex);
        defer _ = c.pthread_mutex_unlock(&state_mutex);
        pool_map.put(pool, .{ .data = pool_data, .size = @intCast(size) }) catch {};
    }
    return pool;
}

pub fn wl_shm_pool_create_buffer(pool: *WlShmPool, offset: i32, width: i32, height: i32, stride: i32, format: u32) *WlBuffer {
    ensure_init();
    const real_fn = @as(*const fn (*WlShmPool, i32, i32, i32, i32, u32)  *WlBuffer, @ptrCast(get_real_fn("wl_shm_pool_create_buffer") orelse @panic("dlsym failed")));
    const buffer = real_fn(pool, offset, width, height, stride, format);
    _ = c.pthread_mutex_lock(&state_mutex);
    defer _ = c.pthread_mutex_unlock(&state_mutex);
    if (pool_map.get(pool)) |pool_info| {
        const buf_info: BufferInfo = .{
            .pool_data = pool_info.data,
            .offset = @intCast(offset),
            .width = @intCast(width),
            .height = @intCast(height),
            .stride = @intCast(stride),
            .format = format,
        };
        buffer_map.put(buffer, buf_info) catch {};
    }
    return buffer;
}

pub fn wl_surface_attach(surface: *WlSurface, buffer: ?*WlBuffer, x: i32, y: i32)  void {
    ensure_init();
    const real_fn = @as(*const fn (*WlSurface, ?*WlBuffer, i32, i32)  void, @ptrCast(get_real_fn("wl_surface_attach") orelse @panic("dlsym failed")));
    real_fn(surface, buffer, x, y);
    _ = c.pthread_mutex_lock(&state_mutex);
    defer _ = c.pthread_mutex_unlock(&state_mutex);
    if (buffer) |buf| {
        surface_map.put(surface, buf) catch {};
    } else {
        _ = surface_map.remove(surface);
    }
}

pub fn wl_surface_commit(surface: *WlSurface)  void {
    ensure_init();
    const real_fn = @as(*const fn (*WlSurface)  void, @ptrCast(get_real_fn("wl_surface_commit") orelse @panic("dlsym failed")));
    _ = c.pthread_mutex_lock(&state_mutex);
    defer _ = c.pthread_mutex_unlock(&state_mutex);
    if (surface_map.get(surface)) |buffer| {
        if (buffer_map.get(buffer)) |buf_info| {
            const pixel_start = buf_info.pool_data + buf_info.offset;
            shm_out.write_frame(buf_info.width, buf_info.height, buf_info.stride, buf_info.format, pixel_start);
        }
    }
    real_fn(surface);
}
