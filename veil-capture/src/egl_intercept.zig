const std = @import("std");
const shm_out = @import("shm_out.zig");
const c = @cImport({
    @cInclude("EGL/egl.h");
    @cInclude("GL/gl.h");
    @cInclude("dlfcn.h");
    @cInclude("string.h");
    @cInclude("unistd.h");
});

var gpa = std.heap.GeneralPurposeAllocator(.{}){};
var alloc: std.mem.Allocator = undefined;
var egl_initialized: bool = false;

fn ensure_egl_init() void {
    if (egl_initialized) return;
    egl_initialized = true;
    alloc = gpa.allocator();
}

fn get_real_fn(comptime name: []const u8) ?*const anyopaque {
    return c.dlsym(c.RTLD_NEXT, @ptrCast(name));
}

pub fn eglSwapBuffers(display: c.EGLDisplay, surface: c.EGLSurface) c.EGLBoolean {
    ensure_egl_init();
    const real_fn = @as(*const fn (c.EGLDisplay, c.EGLSurface)  c.EGLBoolean,
        @ptrCast(get_real_fn("eglSwapBuffers") orelse return 0));
    var width: c.EGLint = 0;
    var height: c.EGLint = 0;
    const egl_query = @as(*const fn (c.EGLDisplay, c.EGLSurface, c.EGLint, [*]c.EGLint)  c.EGLBoolean,
        @ptrCast(get_real_fn("eglQuerySurface") orelse return real_fn(display, surface)));
    _ = egl_query(display, surface, c.EGL_WIDTH, @ptrCast(&width));
    _ = egl_query(display, surface, c.EGL_HEIGHT, @ptrCast(&height));
    if (width > 0 and height > 0) {
        const stride = @as(u32, @intCast(width)) * 4;
        const pixel_count = stride * @as(u32, @intCast(height));
        const pixels = alloc.alloc(u8, pixel_count) catch {
            return real_fn(display, surface);
        };
        defer alloc.free(pixels);
        const gl_get_error = @as(*const fn ()  c.GLenum,
            @ptrCast(get_real_fn("glGetError") orelse return real_fn(display, surface)));
        _ = gl_get_error();
        const gl_read_pixels = @as(*const fn (c.GLint, c.GLint, c.GLsizei, c.GLsizei, c.GLenum, c.GLenum, ?*anyopaque)  void,
            @ptrCast(get_real_fn("glReadPixels") orelse return real_fn(display, surface)));
        gl_read_pixels(0, 0, width, height, c.GL_RGBA, c.GL_UNSIGNED_BYTE, pixels.ptr);
        flip_image_vertical(pixels.ptr, @intCast(width), @intCast(height), 4);
        shm_out.write_frame(@intCast(width), @intCast(height), stride, c.GL_RGBA, pixels.ptr);
    }
    return real_fn(display, surface);
}

fn flip_image_vertical(data: [*]u8, width: u32, height: u32, bytes_per_pixel: u32) void {
    const row_size = width * bytes_per_pixel;
    const temp_row = std.heap.c_allocator.alloc(u8, row_size) catch return;
    defer std.heap.c_allocator.free(temp_row);
    var top: u32 = 0;
    var bottom: u32 = height - 1;
    while (top < bottom) {
        const top_row = data + (top * row_size);
        const bottom_row = data + (bottom * row_size);
        std.mem.copyForwards(u8, temp_row, top_row[0..row_size]);
        std.mem.copyForwards(u8, top_row[0..row_size], bottom_row[0..row_size]);
        std.mem.copyForwards(u8, bottom_row[0..row_size], temp_row[0..row_size]);
        top += 1;
        bottom -= 1;
    }
}
