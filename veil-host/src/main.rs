//! Standalone runner: spawn a Wayland app inside veil-host and render
//! its frames via auto-detected output backend (terminal or DRM/KMS).
//!
//! Usage:
//!   veil-host run weston-terminal
//!   veil-host run -d foot
//!   veil-host run -s wayland-veil-0 firefox

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use veil_host::input_backend::{self, InputCtx, InputGeometry};
use veil_host::{Host, HostConfig};
use veil_config::detect_quality;

const VERSION: &str  = env!("CARGO_PKG_VERSION");

/// Persistent debug-log path (NOT /tmp — that's tmpfs and is wiped by the
/// reboot after a hard lock, taking the crash evidence with it).
/// `$XDG_STATE_HOME/veil/veil.log`, falling back to `~/.local/state/...`.
fn log_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home).join(".local/state")
        });
    base.join("veil").join("veil.log")
}

fn print_help() {
    println!("veil-host {VERSION} — nested Wayland compositor → terminal renderer");
    println!();
    println!("USAGE");
    println!("  veil-host <subcommand> [flags] [args]");
    println!();
    println!("SUBCOMMANDS");
    println!("  run <command> [args...]   Launch a GUI app inside the compositor");
    println!("  probe                     Show terminal capabilities and resolved config");
    println!("  list-modes                List all render modes and which one would be chosen");
    println!();
    println!("RUN FLAGS");
    println!("  -a, --append              Add the app to an already-running veil instance");
    println!("                            (launches against its socket; works cross-VT)");
    println!("  -m, --mode <mode>         Force a render mode (see list-modes for options)");
    println!("  -d, --debug               Redirect all logs to $XDG_STATE_HOME/veil/veil.log");
    println!("  -w, --width <px>          Override compositor width  (default: cols × 8)");
    println!("  -h, --height <px>         Override compositor height (default: rows × 16)");
    println!("  -s, --socket <name>       Wayland socket name (default: wayland-veil-0)");
    println!("      --stats               Show fps / frame-size bar on the bottom row");
    println!();
    println!("GLOBAL FLAGS");
    println!("  -v, --version             Print version and exit");
    println!("      --help                Print this help and exit");
    println!();
    println!("EXAMPLES");
    println!("  veil-host run thunar");
    println!("  veil-host run -a dolphin          # add to a running instance (any VT)");
    println!("  veil-host run -d -m halfblock firefox");
    println!("  veil-host run --stats nautilus");
    println!("  veil-host probe");
    println!("  veil-host list-modes");
    println!();
    println!("CONFIG");
    println!("  Place config.lua at ./config.lua or ~/.config/veil/config.lua");
    println!("    quality = \"auto\"   -- auto | kitty | pixel | ascii | ascii_edge");
    println!("    fps     = 60       -- compositor frame rate cap");
}

fn main() -> std::io::Result<()> {
    let mut raw_args = std::env::args().skip(1);
    let subcmd = raw_args.next().unwrap_or_default();

    match subcmd.as_str() {
        "-v" | "--version"  => { println!("veil-host {VERSION}"); return Ok(()); }
        "--help"            => { print_help(); return Ok(()); }
        "probe"             => return cmd_probe(),
        "list-modes"        => { cmd_list_modes(); return Ok(()); }
        "run"               => {}
        _                   => { eprintln!("unknown subcommand: {subcmd:?}"); eprintln!("run 'veil-host --help' for usage"); std::process::exit(2); }
    }

    // ── `run` subcommand ──────────────────────────────────────────────────────
    let mut cfg        = HostConfig::default();
    let mut debug      = false;
    let mut spawn: Vec<String> = Vec::new();
    let mut explicit_size = false;
    let mut append     = false;

    while let Some(a) = raw_args.next() {
        if !spawn.is_empty() { spawn.push(a); continue; }
        match a.as_str() {
            "--help"          => { print_help(); return Ok(()); }
            "-a" | "--append" => { append = true; }
            "-d" | "--debug"  => { debug = true; cfg.wayland_debug = true; }
            "-w" | "--width"  => {
                cfg.width = raw_args.next().and_then(|s| s.parse().ok()).unwrap_or(cfg.width);
                explicit_size = true;
            }
            "-h" | "--height" => {
                cfg.height = raw_args.next().and_then(|s| s.parse().ok()).unwrap_or(cfg.height);
                explicit_size = true;
            }
            "-s" | "--socket" => {
                cfg.socket_name = raw_args.next().unwrap_or_else(|| { eprintln!("--socket requires a value"); std::process::exit(2); });
            }
            other if other.starts_with('-') => { eprintln!("unknown flag: {other}\nrun 'veil-host --help' for usage"); std::process::exit(2); }
            cmd => { spawn.push(cmd.to_string()); }
        }
    }
    if spawn.is_empty() { eprintln!("error: 'run' requires a command\nrun 'veil-host --help' for usage"); std::process::exit(2); }

    // --append: don't start a compositor — launch the app against an
    // already-running veil instance's socket and exit. Works cross-VT because
    // the socket lives in the shared per-user $XDG_RUNTIME_DIR.
    if append {
        if let Err(e) = attach(&cfg.socket_name, &spawn) {
            eprintln!("[veil-host] {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    cfg.spawn = Some(spawn);

    // ── Debug mode: redirect stderr into the persistent log so it doesn't corrupt output.
    if debug { init_debug_log()?; }
    init_tracing(debug);

    // Size compositor to match actual terminal pixel area unless user gave explicit dims.
    // Assume 8×16 px per cell — the most common monospace glyph box.
    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    if !explicit_size {
        if let Some((pw, ph)) = term_pixel_size() {
            cfg.width  = pw;
            cfg.height = ph;
        } else {
            cfg.width  = term_cols as u32 * 8;
            cfg.height = term_rows as u32 * 16;
        }
    }

    eprintln!(
        "[veil-host] socket={} size={}x{} spawn={:?} debug={}",
        cfg.socket_name, cfg.width, cfg.height, cfg.spawn, debug
    );

    let comp_w = cfg.width;
    let comp_h = cfg.height;
    // Shared geometry for pointer mapping — updated on resize events.
    let (init_cols, init_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let geom = InputGeometry::new(init_cols, init_rows, comp_w, comp_h);
    let host   = Host::spawn(cfg)?;

    // ── SIGINT handler: if ctrl-c slips past raw mode (eg. via `kill -INT`
    //    from another shell), still tear down cleanly.
    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        let s = host.stop_flag();
        let _ = ctrlc_set(move || {
            // Restore the console immediately in case clean teardown stalls —
            // a wedged frame loop must never leave the VT black.
            veil_host::vt::emergency_restore();
            r.store(false, Ordering::Relaxed);
            s.store(true, Ordering::Relaxed);
        });
    }

    // ── input thread: auto-detected backend (crossterm terminal | evdev TTY).
    //    Terminal setup is handled by TerminalOutput when in terminal mode.
    {
        let backend = input_backend::detect();
        let ctx = InputCtx {
            tx:        host.input_sender(),
            running:   running.clone(),
            host_stop: host.stop_flag(),
            geom:      geom.clone(),
        };
        std::thread::spawn(move || backend.run(ctx));
    }

    // ── Create output backend (auto-detect terminal vs DRM/KMS) ────────────────
    let mut output = veil_host::output::detect()?;
    let (out_w, out_h) = output.get_size();
    eprintln!("[veil-host] output backend initialized: {}x{}", out_w, out_h);

    // If the output (e.g. DRM display mode) differs from the size the
    // compositor was spawned at, retarget it so frames arrive at native res
    // instead of being clipped/letterboxed.
    if (out_w, out_h) != (comp_w, comp_h) {
        geom.comp_w.store(out_w, Ordering::Relaxed);
        geom.comp_h.store(out_h, Ordering::Relaxed);
        let _ = host.input_sender().send(
            veil_host::InputCmd::Resize { width: out_w, height: out_h },
        );
        eprintln!("[veil-host] retargeting compositor to output size {out_w}x{out_h}");
    }

    // ── frame loop ────────────────────────────────────────────────────────────
    let mut fps_frame_count = 0u32;
    let mut fps_last        = std::time::Instant::now();

    while running.load(Ordering::Relaxed) {
        let mut frame = match host.frames().recv_timeout(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // Drain the channel — skip to latest frame if compositor is ahead.
        while let Ok(f) = host.frames().try_recv() { frame = f; }

        // Render via output backend
        output.render_frame(&frame.rgba, frame.width, frame.height)?;

        // FPS stats logging every second
        fps_frame_count += 1;
        let elapsed = fps_last.elapsed();
        if elapsed.as_secs_f32() >= 1.0 {
            let fps = fps_frame_count as f32 / elapsed.as_secs_f32();
            eprintln!("[veil-host] fps: {:.0}  compositor: {}x{}px", fps, frame.width, frame.height);
            fps_frame_count = 0;
            fps_last = std::time::Instant::now();
        }
    }

    Ok(())
}

/// Launch `argv` as a client of an already-running veil instance, then return.
/// No IPC with the running process — we just exec the app pointed at its
/// Wayland socket in `$XDG_RUNTIME_DIR`, detached from this TTY's session so it
/// survives when this shell exits (the veil instance may be on another VT).
fn attach(socket: &str, argv: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;

    let runtime = std::env::var("XDG_RUNTIME_DIR").map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "XDG_RUNTIME_DIR unset — can't locate the veil socket",
        )
    })?;
    let sock_path = std::path::Path::new(&runtime).join(socket);
    // Probe by actually connecting — a stale socket file from an exited
    // instance still passes an existence check but isn't listening. If we
    // can't connect, there's no live instance to attach to (so we don't
    // launch a client into the void, where it may fall back to X11).
    if let Err(e) = std::os::unix::net::UnixStream::connect(&sock_path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            format!(
                "no live veil instance at {} ({e}) — start one with `veil-host run …` first",
                sock_path.display()
            ),
        ));
    }

    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.env("WAYLAND_DISPLAY", socket);
    // Detach into a new session: no controlling TTY, so closing this shell
    // won't SIGHUP the client. It's reparented to init and belongs to veil now.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let child = cmd.spawn().map_err(|e| {
        std::io::Error::new(e.kind(), format!("spawn {:?}: {e}", argv[0]))
    })?;
    println!("[veil-host] attached {argv:?} (pid {}) → {socket}", child.id());
    Ok(())
}

fn term_pixel_size() -> Option<(u32, u32)> {
    let mut winsz: libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut winsz) };
    if ret == 0 && winsz.ws_xpixel > 0 && winsz.ws_ypixel > 0 {
        Some((winsz.ws_xpixel as u32, winsz.ws_ypixel as u32))
    } else {
        None
    }
}

fn dirs_config() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home).join(".config")
        })
        .join("veil")
}

// ─── list-modes ───────────────────────────────────────────────────────────────

fn cmd_list_modes() {
    let detected = detect_quality();

    println!("RENDER MODES");
    println!();

    let modes = [
        ("kitty",      "Kitty Graphics Protocol — native pixel images, best quality",        "$TERM=xterm-kitty or WezTerm"),
        ("halfblock",  "Unicode ▀ half-blocks with 24-bit truecolor, 2× vertical res",      "$COLORTERM=truecolor or 24bit"),
        ("ascii",      "Luma-mapped ASCII characters, works in any terminal",               "any"),
        ("ascii-edge", "Luma with hysteresis edge-detection, sharper than ascii",           "any"),
    ];

    for (name, desc, req) in &modes {
        let marker = if name == &detected.as_str() {
            " ◀ auto-selected"
        } else {
            ""
        };
        println!("  {name:<12}{desc}{marker}");
        println!("  {:<12}requires: {req}", "");
        println!();
    }

    println!("Output backend auto-detects based on environment (terminal vs DRM/KMS).");
}

// ─── probe ────────────────────────────────────────────────────────────────────

fn cmd_probe() -> std::io::Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let pixel_dims = term_pixel_size();
    let (comp_w, comp_h) = pixel_dims.unwrap_or((cols as u32 * 8, rows as u32 * 16));
    let pixel_source = if pixel_dims.is_some() { "TIOCGWINSZ" } else { "cols×8, rows×16 (estimate)" };

    let term      = std::env::var("TERM").unwrap_or_else(|_| "unknown".into());
    let colorterm = std::env::var("COLORTERM").unwrap_or_else(|_| "unset".into());
    let wayland   = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "unset".into());
    let display   = std::env::var("DISPLAY").unwrap_or_else(|_| "unset".into());

    let config_path = [
        std::path::PathBuf::from("config.lua"),
        dirs_config().join("config.lua"),
    ]
    .iter()
    .find(|p| p.exists())
    .cloned();

    let vcfg = config_path.as_ref()
        .map(|p| veil_config::load(p))
        .unwrap_or_default();

    let detected   = detect_quality();
    let ssh_mode = std::env::var("SSH_CLIENT").is_ok() || std::env::var("SSH_TTY").is_ok();
    let compositor_mode = wayland != "unset" || display != "unset";

    println!("terminal        : {term}");
    println!("colorterm       : {colorterm}");
    println!("term size       : {cols}x{rows} cells");
    println!("compositor      : {comp_w}x{comp_h} px  ({pixel_source})");
    println!("detected quality: {detected:?}");
    println!("config quality  : {}", format!("{:?}", vcfg.quality));
    println!("fps             : {}", vcfg.fps);
    println!("gpu_render      : {}", if vcfg.gpu_render { "on (default)" } else { "off" });
    println!("config file     : {}", config_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none (using defaults)".into()));
    println!("WAYLAND_DISPLAY : {wayland}");
    println!("DISPLAY         : {display}");
    println!("SSH_CLIENT      : {}", if ssh_mode { "yes (using terminal output)" } else { "no" });
    println!("output backend  : {}", if compositor_mode || ssh_mode { "terminal" } else { "DRM/KMS (if available)" });
    println!("socket (default): wayland-veil-0");
    Ok(())
}

// ─── signal handler ───────────────────────────────────────────────────────────

/// Install a SIGINT handler. The closure must be Send + 'static and is
/// stashed in a static slot — first call wins, subsequent calls no-op.
fn ctrlc_set<F: FnMut() + Send + 'static>(mut f: F) -> std::io::Result<()> {
    use std::sync::Mutex;
    static HOOK: Mutex<Option<Box<dyn FnMut() + Send>>> = Mutex::new(None);

    extern "C" fn handler(_sig: libc::c_int) {
        if let Ok(mut g) = HOOK.lock() {
            if let Some(cb) = g.as_mut() { cb(); }
        }
    }

    *HOOK.lock().unwrap() = Some(Box::new(move || f()));
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        if libc::sigaction(libc::SIGINT,  &sa, std::ptr::null_mut()) < 0
        || libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut()) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

// ─── debug log ────────────────────────────────────────────────────────────────

fn init_debug_log() -> std::io::Result<()> {
    let path = log_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = OpenOptions::new().create(true).append(true).open(&path)?;
    let fd = f.as_raw_fd();
    unsafe {
        // Redirect fd 2 (stderr) into the log file. We deliberately leak the
        // File so the underlying fd stays alive for the process lifetime.
        if libc::dup2(fd, 2) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    std::mem::forget(f);
    eprintln!("\n[veil-host] ── debug log opened ── ({})", path.display());
    Ok(())
}

fn init_tracing(debug: bool) {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let filter = std::env::var("VEIL_LOG")
            .ok()
            .or_else(|| if debug { Some("debug".into()) } else { None });
        if let Some(f) = filter {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_new(f)
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .with_writer(std::io::stderr)
                .init();
        }
    });
}

