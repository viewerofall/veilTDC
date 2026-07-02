//! Virtual-terminal takeover for bare-TTY DRM mode.
//!
//! When we mode-set a CRTC on a bare TTY we MUST take the VT out of text mode
//! first (`KD_GRAPHICS`). Otherwise the kernel framebuffer console (fbcon) is
//! still bound to the same CRTC and fights our page-flips — on amdgpu this
//! reliably ends in a GPU ring timeout → reset → hard lock / panic.
//!
//! [`VtGuard`] flips the active VT to graphics mode and restores text mode on
//! drop. Because a crash that skips `Drop` would leave the console black and
//! unusable, we also install a panic hook and fatal-signal handlers that
//! restore text mode from a stashed fd — a last-resort so a bug never leaves
//! the machine wedged.

use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Once;

// linux/kd.h
const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_TEXT: libc::c_int = 0x00;
const KD_GRAPHICS: libc::c_int = 0x01;

/// Raw fd of the controlling VT, published for the emergency restore path.
/// -1 means "not in graphics mode / nothing to restore".
static VT_FD: AtomicI32 = AtomicI32::new(-1);

pub struct VtGuard {
    fd: OwnedFd,
}

impl VtGuard {
    /// Put the controlling terminal into graphics mode. Fails on a pty
    /// (e.g. SSH) where `KDSETMODE` returns ENOTTY — caller should treat that
    /// as "not a real VT" and avoid DRM.
    pub fn acquire() -> io::Result<Self> {
        let raw = unsafe {
            libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC)
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        if unsafe { libc::ioctl(raw, KDSETMODE, KD_GRAPHICS) } < 0 {
            return Err(io::Error::last_os_error());
        }

        VT_FD.store(raw, Ordering::SeqCst);
        install_emergency_handlers();
        eprintln!("[veil-host] VT → graphics mode (fbcon suspended)");
        Ok(VtGuard { fd })
    }
}

impl Drop for VtGuard {
    fn drop(&mut self) {
        let raw = self.fd.as_raw_fd();
        unsafe {
            libc::ioctl(raw, KDSETMODE, KD_TEXT);
            // Discard anything typed while we owned the screen so it can't run
            // in the shell once text mode returns.
            libc::tcflush(raw, libc::TCIFLUSH);
        }
        VT_FD.store(-1, Ordering::SeqCst);
        eprintln!("[veil-host] VT → text mode");
    }
}

/// Restore the VT to text mode immediately, from anywhere (signal handler,
/// panic hook, explicit shutdown). Idempotent and async-signal-safe enough
/// for our purposes (single ioctl + tcflush on a stashed fd).
pub fn emergency_restore() {
    let raw = VT_FD.load(Ordering::SeqCst);
    if raw >= 0 {
        unsafe {
            libc::ioctl(raw, KDSETMODE, KD_TEXT);
            libc::tcflush(raw, libc::TCIFLUSH);
        }
    }
}

fn install_emergency_handlers() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Panic: restore the console, then run the normal hook so the backtrace
        // still prints (into the log).
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            emergency_restore();
            prev(info);
        }));

        // Hard crashes that bypass Drop: restore, reset to default disposition,
        // re-raise so we still get the real fault/coredump.
        for sig in [libc::SIGSEGV, libc::SIGABRT, libc::SIGBUS, libc::SIGILL, libc::SIGFPE] {
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = fatal_handler as *const () as usize;
                libc::sigemptyset(&mut sa.sa_mask);
                sa.sa_flags = libc::SA_RESETHAND;
                libc::sigaction(sig, &sa, std::ptr::null_mut());
            }
        }
    });
}

extern "C" fn fatal_handler(sig: libc::c_int) {
    emergency_restore();
    // SA_RESETHAND restored the default handler; re-raise to get the real crash.
    unsafe {
        libc::raise(sig);
    }
}
