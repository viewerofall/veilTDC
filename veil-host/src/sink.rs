//! Frame output from the host compositor.
//!
//! Sink-agnostic: caller decides what to do with the RGBA. veil-cli
//! encodes to kitty graphics today; the same Frame stream feeds
//! /dev/fb0, sixel, iterm2, DRM dumb buffers tomorrow.

/// One rendered frame of the hosted scene.
///
/// `rgba` is tightly packed `width * height * 4` bytes, R-G-B-A order.
/// Buffer is owned — caller is free to hold it past the next frame.
#[derive(Clone)]
pub struct Frame {
    pub rgba:   Vec<u8>,
    pub width:  u32,
    pub height: u32,
    /// Monotonic frame counter from the compositor. Useful for skip detection.
    pub serial: u64,
}

// TODO: composite hosted-client surfaces into the RGBA buffer.
//   v1: single fullscreen toplevel, copy its shm buffer straight through
//   (with format conversion if not already RGBA).
//   v2: software renderer over multiple surfaces + subsurfaces, damage tracking.
//   v3: pluggable GPU path via smithay::backend::renderer when available.
