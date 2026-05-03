#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <errno.h>

#include <wayland-client.h>
#include "ext-foreign-toplevel-list-v1.h"
#include "ext-image-capture-source-v1.h"
#include "ext-image-copy-capture-v1.h"

#define FRAME_MAGIC 0x56454C43u

/* ── Global compositor state ─────────────────────────────────────────────── */

static struct {
    struct wl_display                                        *display;
    struct wl_registry                                       *registry;
    struct wl_shm                                            *shm;
    struct ext_foreign_toplevel_list_v1                      *toplevel_list;
    struct ext_foreign_toplevel_image_capture_source_manager_v1 *source_mgr;
    struct ext_image_copy_capture_manager_v1                 *capture_mgr;

    const char *target_app_id;
    const char *target_title;

    struct ext_foreign_toplevel_handle_v1 *matched_handle;

    struct ext_image_copy_capture_session_v1 *session;
    uint32_t buf_width;
    uint32_t buf_height;
    uint32_t buf_format;
    int session_done;

    struct wl_shm_pool *pool;
    struct wl_buffer   *buffer;
    void               *shm_data;
    size_t              shm_size;

    int frame_ready;
    int frame_failed;
    int running;
} S;

/* ── Per-toplevel tracking ───────────────────────────────────────────────── */

typedef struct {
    struct ext_foreign_toplevel_handle_v1 *handle;
    char app_id[512];
    char title[512];
} ToplevelUD;

static void tl_app_id(void *data, struct ext_foreign_toplevel_handle_v1 *h, const char *id) {
    ToplevelUD *ud = data; (void)h;
    snprintf(ud->app_id, sizeof(ud->app_id), "%s", id);
}
static void tl_title(void *data, struct ext_foreign_toplevel_handle_v1 *h, const char *t) {
    ToplevelUD *ud = data; (void)h;
    snprintf(ud->title, sizeof(ud->title), "%s", t);
}
static void tl_identifier(void *data, struct ext_foreign_toplevel_handle_v1 *h, const char *id) {
    (void)data; (void)h; (void)id;
}
static void tl_done(void *data, struct ext_foreign_toplevel_handle_v1 *h) {
    ToplevelUD *ud = data; (void)h;
    if (S.matched_handle) return;
    int app_match = (strcmp(ud->app_id, S.target_app_id) == 0);
    int title_match = (!S.target_title || strcmp(ud->title, S.target_title) == 0);
    if (app_match && title_match)
        S.matched_handle = ud->handle;
}
static void tl_closed(void *data, struct ext_foreign_toplevel_handle_v1 *h) {
    ToplevelUD *ud = data; (void)h;
    if (ud->handle == S.matched_handle)
        S.running = 0;
    free(ud);
}

static const struct ext_foreign_toplevel_handle_v1_listener tl_listener = {
    .app_id     = tl_app_id,
    .title      = tl_title,
    .identifier = tl_identifier,
    .done       = tl_done,
    .closed     = tl_closed,
};

/* ── Toplevel list listener ──────────────────────────────────────────────── */

static void list_toplevel(void *data, struct ext_foreign_toplevel_list_v1 *list,
                          struct ext_foreign_toplevel_handle_v1 *handle) {
    (void)data; (void)list;
    ToplevelUD *ud = calloc(1, sizeof(*ud));
    ud->handle = handle;
    ext_foreign_toplevel_handle_v1_add_listener(handle, &tl_listener, ud);
}
static void list_finished(void *data, struct ext_foreign_toplevel_list_v1 *list) {
    (void)data; (void)list;
}

static const struct ext_foreign_toplevel_list_v1_listener list_listener = {
    .toplevel = list_toplevel,
    .finished = list_finished,
};

/* ── Session listener ────────────────────────────────────────────────────── */

static void ses_buffer_size(void *data, struct ext_image_copy_capture_session_v1 *ses,
                            uint32_t w, uint32_t h) {
    (void)data; (void)ses;
    S.buf_width = w; S.buf_height = h;
}
static void ses_shm_format(void *data, struct ext_image_copy_capture_session_v1 *ses, uint32_t fmt) {
    (void)data; (void)ses;
    S.buf_format = fmt;
}
static void ses_dmabuf_device(void *data, struct ext_image_copy_capture_session_v1 *ses,
                              struct wl_array *dev) {
    (void)data; (void)ses; (void)dev;
}
static void ses_dmabuf_format(void *data, struct ext_image_copy_capture_session_v1 *ses,
                              uint32_t fmt, struct wl_array *modifiers) {
    (void)data; (void)ses; (void)fmt; (void)modifiers;
}
static void ses_done(void *data, struct ext_image_copy_capture_session_v1 *ses) {
    (void)data; (void)ses;
    S.session_done = 1;
}
static void ses_stopped(void *data, struct ext_image_copy_capture_session_v1 *ses) {
    (void)data; (void)ses;
    S.running = 0;
}

static const struct ext_image_copy_capture_session_v1_listener ses_listener = {
    .buffer_size   = ses_buffer_size,
    .shm_format    = ses_shm_format,
    .dmabuf_device = ses_dmabuf_device,
    .dmabuf_format = ses_dmabuf_format,
    .done          = ses_done,
    .stopped       = ses_stopped,
};

/* ── Frame listener ──────────────────────────────────────────────────────── */

static void fr_transform(void *data, struct ext_image_copy_capture_frame_v1 *fr, uint32_t t) {
    (void)data; (void)fr; (void)t;
}
static void fr_damage(void *data, struct ext_image_copy_capture_frame_v1 *fr,
                      int32_t x, int32_t y, int32_t w, int32_t h) {
    (void)data; (void)fr; (void)x; (void)y; (void)w; (void)h;
}
static void fr_presentation_time(void *data, struct ext_image_copy_capture_frame_v1 *fr,
                                 uint32_t hi, uint32_t lo, uint32_t ns) {
    (void)data; (void)fr; (void)hi; (void)lo; (void)ns;
}
static void fr_ready(void *data, struct ext_image_copy_capture_frame_v1 *fr) {
    (void)data; (void)fr;
    S.frame_ready = 1;
}
static void fr_failed(void *data, struct ext_image_copy_capture_frame_v1 *fr, uint32_t reason) {
    (void)data; (void)fr; (void)reason;
    S.frame_failed = 1;
}

static const struct ext_image_copy_capture_frame_v1_listener frame_listener = {
    .transform         = fr_transform,
    .damage            = fr_damage,
    .presentation_time = fr_presentation_time,
    .ready             = fr_ready,
    .failed            = fr_failed,
};

/* ── Registry listener ───────────────────────────────────────────────────── */

static void reg_global(void *data, struct wl_registry *reg, uint32_t name,
                       const char *iface, uint32_t ver) {
    (void)data;
    if (!strcmp(iface, wl_shm_interface.name)) {
        S.shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    } else if (!strcmp(iface, ext_foreign_toplevel_list_v1_interface.name)) {
        S.toplevel_list = wl_registry_bind(reg, name, &ext_foreign_toplevel_list_v1_interface, 1);
        ext_foreign_toplevel_list_v1_add_listener(S.toplevel_list, &list_listener, NULL);
    } else if (!strcmp(iface, ext_foreign_toplevel_image_capture_source_manager_v1_interface.name)) {
        S.source_mgr = wl_registry_bind(reg, name,
            &ext_foreign_toplevel_image_capture_source_manager_v1_interface, 1);
    } else if (!strcmp(iface, ext_image_copy_capture_manager_v1_interface.name)) {
        S.capture_mgr = wl_registry_bind(reg, name,
            &ext_image_copy_capture_manager_v1_interface, 1);
    }
    (void)ver;
}
static void reg_global_remove(void *data, struct wl_registry *reg, uint32_t name) {
    (void)data; (void)reg; (void)name;
}

static const struct wl_registry_listener reg_listener = {
    .global        = reg_global,
    .global_remove = reg_global_remove,
};

/* ── SHM buffer allocation ───────────────────────────────────────────────── */

static int alloc_buffer(uint32_t w, uint32_t h) {
    uint32_t stride = w * 4;
    size_t   size   = (size_t)stride * h;

    if (S.shm_data) { munmap(S.shm_data, S.shm_size); S.shm_data = NULL; }
    if (S.buffer)   { wl_buffer_destroy(S.buffer);    S.buffer   = NULL; }
    if (S.pool)     { wl_shm_pool_destroy(S.pool);    S.pool     = NULL; }

    int fd = memfd_create("veil_scrcpy", MFD_CLOEXEC);
    if (fd < 0) return -1;
    if (ftruncate(fd, (off_t)size) < 0) { close(fd); return -1; }

    S.shm_data = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (S.shm_data == MAP_FAILED) { close(fd); S.shm_data = NULL; return -1; }
    S.shm_size = size;

    S.pool   = wl_shm_create_pool(S.shm, fd, (int32_t)size);
    S.buffer = wl_shm_pool_create_buffer(S.pool, 0, (int32_t)w, (int32_t)h,
                                         (int32_t)stride, S.buf_format);
    close(fd);
    return 0;
}

/* ── Frame output ────────────────────────────────────────────────────────── */

static void write_all(const void *buf, size_t n) {
    const uint8_t *p = buf;
    while (n > 0) {
        ssize_t r = write(STDOUT_FILENO, p, n);
        if (r <= 0) exit(0);   /* broken pipe — reader exited cleanly */
        p += r; n -= (size_t)r;
    }
}

static void emit_frame(uint32_t w, uint32_t h, const uint8_t *src) {
    /* Convert ARGB8888/XRGB8888 LE (B,G,R,A bytes) → RGBA8888 (R,G,B,A bytes) */
    uint32_t n    = w * h;
    uint8_t *rgba = malloc((size_t)n * 4);
    if (!rgba) return;

    int has_alpha = (S.buf_format == WL_SHM_FORMAT_ARGB8888);
    for (uint32_t i = 0; i < n; i++) {
        const uint8_t *s = src + i * 4;
        uint8_t       *d = rgba + i * 4;
        d[0] = s[2];                         /* R */
        d[1] = s[1];                         /* G */
        d[2] = s[0];                         /* B */
        d[3] = has_alpha ? s[3] : 0xFFu;    /* A */
    }

    uint32_t hdr[3] = { FRAME_MAGIC, w, h };
    write_all(hdr, sizeof(hdr));
    write_all(rgba, (size_t)n * 4);
    free(rgba);
}

/* ── Main ────────────────────────────────────────────────────────────────── */

int main(int argc, char *argv[]) {
    if (argc < 2) {
        fprintf(stderr, "usage: veil-screencopy <app_id> [title]\n");
        return 1;
    }

    S.target_app_id = argv[1];
    S.target_title  = (argc >= 3) ? argv[2] : NULL;
    S.running       = 1;

    S.display = wl_display_connect(NULL);
    if (!S.display) {
        fprintf(stderr, "veil-screencopy: failed to connect to Wayland\n");
        return 1;
    }

    S.registry = wl_display_get_registry(S.display);
    wl_registry_add_listener(S.registry, &reg_listener, NULL);

    /* Two round-trips: first gets globals + toplevel handles,
       second gets all handle events (app_id, title, done). */
    wl_display_roundtrip(S.display);
    wl_display_roundtrip(S.display);

    if (!S.capture_mgr || !S.source_mgr || !S.shm) {
        fprintf(stderr, "veil-screencopy: compositor missing ext-image-copy-capture-v1\n");
        return 2;
    }
    if (!S.toplevel_list) {
        fprintf(stderr, "veil-screencopy: compositor missing ext-foreign-toplevel-list-v1\n");
        return 2;
    }
    if (!S.matched_handle) {
        fprintf(stderr, "veil-screencopy: window '%s' not found\n", S.target_app_id);
        return 3;
    }

    /* Create capture source from the toplevel handle */
    struct ext_image_capture_source_v1 *source =
        ext_foreign_toplevel_image_capture_source_manager_v1_create_source(
            S.source_mgr, S.matched_handle);

    /* Create capture session (no cursor overlay) */
    S.session = ext_image_copy_capture_manager_v1_create_session(S.capture_mgr, source, 0);
    ext_image_copy_capture_session_v1_add_listener(S.session, &ses_listener, NULL);
    ext_image_capture_source_v1_destroy(source);

    /* Wait for session to report buffer dimensions and format */
    while (!S.session_done && S.running) {
        if (wl_display_dispatch(S.display) < 0) { S.running = 0; break; }
    }
    if (!S.session_done) {
        fprintf(stderr, "veil-screencopy: session failed to initialise\n");
        return 1;
    }

    if (alloc_buffer(S.buf_width, S.buf_height) < 0) {
        fprintf(stderr, "veil-screencopy: SHM buffer allocation failed\n");
        return 1;
    }

    /* Capture loop */
    while (S.running) {
        S.frame_ready  = 0;
        S.frame_failed = 0;

        struct ext_image_copy_capture_frame_v1 *frame =
            ext_image_copy_capture_session_v1_create_frame(S.session);
        ext_image_copy_capture_frame_v1_add_listener(frame, &frame_listener, NULL);
        ext_image_copy_capture_frame_v1_attach_buffer(frame, S.buffer);
        ext_image_copy_capture_frame_v1_damage_buffer(frame, 0, 0, S.buf_width, S.buf_height);
        ext_image_copy_capture_frame_v1_capture(frame);
        wl_display_flush(S.display);

        while (!S.frame_ready && !S.frame_failed && S.running) {
            if (wl_display_dispatch(S.display) < 0) { S.running = 0; break; }
        }

        if (S.frame_ready)
            emit_frame(S.buf_width, S.buf_height, (const uint8_t *)S.shm_data);

        ext_image_copy_capture_frame_v1_destroy(frame);
    }

    if (S.shm_data) munmap(S.shm_data, S.shm_size);
    wl_display_disconnect(S.display);
    return 0;
}
