const std = @import("std");
const intercept = @import("intercept.zig");
const egl = @import("egl_intercept.zig");
const c = @cImport({
    @cInclude("EGL/egl.h");
});

pub export fn wl_shm_create_pool(shm: *intercept.WlShm, fd: i32, size: i32) *intercept.WlShmPool {
    return intercept.wl_shm_create_pool(shm, fd, size);
}

pub export fn wl_shm_pool_create_buffer(pool: *intercept.WlShmPool, offset: i32, width: i32, height: i32, stride: i32, format: u32) *intercept.WlBuffer {
    return intercept.wl_shm_pool_create_buffer(pool, offset, width, height, stride, format);
}

pub export fn wl_surface_attach(surface: *intercept.WlSurface, buffer: ?*intercept.WlBuffer, x: i32, y: i32) void {
    return intercept.wl_surface_attach(surface, buffer, x, y);
}

pub export fn wl_surface_commit(surface: *intercept.WlSurface) void {
    return intercept.wl_surface_commit(surface);
}

pub export fn eglSwapBuffers(display: c.EGLDisplay, surface: c.EGLSurface) c.EGLBoolean {
    return egl.eglSwapBuffers(display, surface);
}
