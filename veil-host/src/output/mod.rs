//! Output abstraction: render RGBA frames to terminal or framebuffer.
//!
//! Two implementations:
//! - TerminalOutput: Kitty graphics protocol, halfblock, ASCII (current terminal rendering)
//! - DrmOutput: Direct framebuffer via DRM/KMS on bare TTY

pub mod terminal;
pub mod drm;

use std::io;

pub use terminal::TerminalOutput;
pub use drm::DrmOutput;

/// Trait for output backends.
///
/// Not `Send`: `DrmOutput` owns a libseat handle (a raw pointer) and lives on
/// the main thread for its whole lifetime. The frame loop never moves it.
pub trait OutputBackend {
    /// Render an RGBA frame. Blocks until complete or error.
    fn render_frame(&mut self, rgba: &[u8], width: u32, height: u32) -> io::Result<()>;

    /// Get current output dimensions in pixels.
    fn get_size(&self) -> (u32, u32);

    /// Called when VT is being switched away (suspend output, release resources).
    /// Only relevant for DrmOutput; TerminalOutput can no-op this.
    fn on_vt_switch(&mut self, switch_in: bool) -> io::Result<()>;
}

#[derive(PartialEq)]
enum Force {
    Drm,
    Terminal,
}

/// Auto-detect best output backend for current environment.
///
/// `VEIL_OUTPUT=drm` forces DRM/KMS unconditionally — even nested under a
/// real compositor — and errors loudly if it can't init, instead of
/// silently falling back; `VEIL_OUTPUT=terminal` forces terminal the same
/// way. Both are an explicit one-off override and win over everything else.
///
/// `pref` is config.lua's `output` field. Unlike the env var, `pref ==
/// OutputPref::Drm` does NOT override the nested-compositor check below —
/// it only replaces the SSH-session check with an unconditional DRM/KMS
/// attempt on what still looks like a bare TTY. It's a persistent "trust
/// bare-TTY sessions to have real GPU hardware" setting, not a sledgehammer:
/// running `veil-host run` under Niri with `output = "drm"` in config still
/// correctly uses terminal output, exactly like `output = "auto"` would.
pub fn detect(pref: veil_config::OutputPref) -> io::Result<Box<dyn OutputBackend>> {
    let force = match std::env::var("VEIL_OUTPUT").ok().as_deref() {
        Some("drm") | Some("kms")  => Some(Force::Drm),
        Some("terminal") | Some("term") => Some(Force::Terminal),
        _ => None,
    };

    if force == Some(Force::Terminal) {
        eprintln!("[veil-host] VEIL_OUTPUT=terminal → terminal output");
        return Ok(Box::new(TerminalOutput::new()?));
    }

    if force == Some(Force::Drm) {
        eprintln!("[veil-host] VEIL_OUTPUT=drm → forcing DRM/KMS");
        return Ok(Box::new(DrmOutput::new()?));
    }

    // If running under a Wayland/X11 compositor, use terminal output. This
    // MUST come before the config `output` preference — nested mode is never
    // a DRM/KMS candidate no matter what config.lua says.
    if std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("DISPLAY").is_ok() {
        eprintln!("[veil-host] detected compositor via env, using terminal output");
        return Ok(Box::new(TerminalOutput::new()?));
    }

    // config.lua `output = "terminal"`: same effect as VEIL_OUTPUT=terminal,
    // just persistent. Checked here (post compositor-check) so it can't
    // fight the nested-mode detection above, though in practice it wouldn't.
    if pref == veil_config::OutputPref::Terminal {
        eprintln!("[veil-host] config.lua output=terminal → terminal output");
        return Ok(Box::new(TerminalOutput::new()?));
    }

    // config.lua `output = "drm"`: skip the SSH-session check (the one the
    // heuristic gets wrong most often — e.g. a stale SSH_TTY, or genuinely
    // SSH'd into your own box to reach a bare TTY) and go straight for
    // DRM/KMS, loud on failure rather than silently falling back.
    if pref == veil_config::OutputPref::Drm {
        eprintln!("[veil-host] config.lua output=drm → forcing DRM/KMS (bare TTY assumed)");
        return Ok(Box::new(DrmOutput::new()?));
    }

    // If SSH session, use terminal output
    if std::env::var("SSH_CLIENT").is_ok() || std::env::var("SSH_TTY").is_ok() {
        eprintln!("[veil-host] detected SSH session, using terminal output");
        return Ok(Box::new(TerminalOutput::new()?));
    }

    // Try DRM/KMS on bare TTY
    match DrmOutput::new() {
        Ok(drm) => {
            eprintln!("[veil-host] DRM/KMS available, using bare-metal framebuffer output");
            Ok(Box::new(drm))
        }
        Err(e) => {
            // Surface the REAL reason — this is almost always the thing to fix
            // (seat backend, /dev/dri perms, no connected display).
            eprintln!("[veil-host] DRM/KMS unavailable: {e}");
            if force == Some(Force::Drm) {
                return Err(e); // user explicitly asked for DRM; don't hide it
            }
            eprintln!("[veil-host] falling back to terminal output");
            Ok(Box::new(TerminalOutput::new()?))
        }
    }
}
