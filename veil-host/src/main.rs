//! Standalone runner: spawn a Wayland app inside veil-host and render
//! its frames into the host terminal via the Kitty graphics protocol.
//!
//! Usage:
//!   veil-host -- weston-terminal
//!   veil-host -d -- foot
//!   veil-host wayland-veil-foo -- weston-terminal

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    cursor,
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{self, ClearType},
};
use std::fmt::Write as _;
use veil_host::{Host, HostConfig, InputCmd};
use veil_render::{rgba_to_halfblocks, compute_luma, luma_to_chars, apply_hysteresis, render_kitty_frame, KITTY_DELETE};
use veil_config::{Quality, detect_quality};

#[derive(Copy, Clone, Debug)]
enum RenderMode { Kitty, Halfblock, Ascii, AsciiEdge }

fn quality_to_mode(q: Quality) -> RenderMode {
    match q {
        Quality::Kitty                => RenderMode::Kitty,
        Quality::Pixel                => RenderMode::Halfblock,
        Quality::AsciiLuma            => RenderMode::Ascii,
        Quality::AsciiEdge            => RenderMode::AsciiEdge,
        Quality::Sixel | Quality::Auto => RenderMode::Kitty,
    }
}

const LOG_PATH: &str = "/tmp/veil.log";

fn usage() -> ! {
    eprintln!("usage:");
    eprintln!("  veil-host run [-d] [-w W] [-h H] [-m kitty|halfblock|ascii|ascii-edge] [-s SOCKET] <command> [args...]");
    eprintln!("  veil-host probe");
    std::process::exit(2);
}

fn main() -> std::io::Result<()> {
    let mut raw_args = std::env::args().skip(1);
    let subcmd = raw_args.next().unwrap_or_default();

    match subcmd.as_str() {
        "probe" => return cmd_probe(),
        "run"   => {}
        _       => usage(),
    }

    // ── `run` subcommand ──────────────────────────────────────────────────────
    let mut cfg        = HostConfig::default();
    let mut debug      = false;
    let mut mode_override: Option<RenderMode> = None;
    let mut spawn: Vec<String> = Vec::new();
    let mut explicit_size = false;

    while let Some(a) = raw_args.next() {
        if !spawn.is_empty() { spawn.push(a); continue; }
        match a.as_str() {
            "-d" | "--debug" => { debug = true; cfg.wayland_debug = true; }
            "-w" | "--width"  => {
                cfg.width = raw_args.next().and_then(|s| s.parse().ok()).unwrap_or(cfg.width);
                explicit_size = true;
            }
            "-h" | "--height" => {
                cfg.height = raw_args.next().and_then(|s| s.parse().ok()).unwrap_or(cfg.height);
                explicit_size = true;
            }
            "-m" | "--mode" => match raw_args.next().as_deref() {
                Some("kitty")      => mode_override = Some(RenderMode::Kitty),
                Some("halfblock")  => mode_override = Some(RenderMode::Halfblock),
                Some("ascii")      => mode_override = Some(RenderMode::Ascii),
                Some("ascii-edge") => mode_override = Some(RenderMode::AsciiEdge),
                other => { eprintln!("unknown mode: {other:?}"); usage(); }
            },
            "-s" | "--socket" => {
                cfg.socket_name = raw_args.next().unwrap_or_else(|| usage());
            }
            other if other.starts_with('-') => { eprintln!("unknown flag: {other}"); usage(); }
            cmd => { spawn.push(cmd.to_string()); }
        }
    }
    if spawn.is_empty() { eprintln!("error: run requires a command"); usage(); }
    cfg.spawn = Some(spawn);

    // Load config.lua — ./config.lua then ~/.config/veil/config.lua.
    let vcfg = [
        std::path::PathBuf::from("config.lua"),
        dirs_config().join("config.lua"),
    ]
    .iter()
    .find(|p| p.exists())
    .map(|p| veil_config::load(p))
    .unwrap_or_default();

    // Render mode: CLI flag wins, then config quality, then auto-detect.
    let mode = mode_override.unwrap_or_else(|| {
        let q = if vcfg.quality == veil_config::Quality::Auto {
            detect_quality()
        } else {
            vcfg.quality.clone()
        };
        quality_to_mode(q)
    });

    cfg.fps = vcfg.fps;

    // Size compositor to match actual terminal pixel area unless user gave explicit dims.
    // Assume 8×16 px per cell — the most common monospace glyph box.
    let (term_cols, term_rows) = terminal::size().unwrap_or((80, 24));
    if !explicit_size {
        cfg.width  = term_cols as u32 * 8;
        cfg.height = term_rows as u32 * 16;
    }

    // ── Debug mode: redirect stderr into /tmp/veil.log so it doesn't corrupt
    //    kitty graphics on stdout.
    if debug { init_debug_log()?; }
    init_tracing(debug);

    eprintln!(
        "[veil-host] socket={} size={}x{} spawn={:?} debug={} mode={:?}",
        cfg.socket_name, cfg.width, cfg.height, cfg.spawn, debug, mode
    );

    let comp_w = cfg.width;
    let comp_h = cfg.height;
    // Track current compositor dims — updated on resize events.
    let cur_w = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(comp_w));
    let cur_h = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(comp_h));
    let host   = Host::spawn(cfg)?;

    // ── SIGINT handler: if ctrl-c slips past raw mode (eg. via `kill -INT`
    //    from another shell), still tear down cleanly.
    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        let s = host.stop_flag();
        let _ = ctrlc_set(move || {
            r.store(false, Ordering::Relaxed);
            s.store(true, Ordering::Relaxed);
        });
    }

    // ── terminal setup ────────────────────────────────────────────────────────
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode()?;
    execute!(
        stdout,
        terminal::EnterAlternateScreen,
        terminal::Clear(ClearType::All),
        cursor::Hide,
        cursor::MoveTo(0, 0),
    )?;
    // Enable only any-event + SGR mouse modes. EnableMouseCapture also enables
    // ?1015h (URXVT extended) which causes kitty to send events in both URXVT
    // and SGR encodings, making crossterm emit spurious duplicate button events
    // (e.g. a physical left click arrives as Down(Left) + Down(Right)).
    stdout.write_all(b"\x1b[?1003h\x1b[?1006h")?;
    let _guard = TermGuard;

    // ── input thread: read crossterm events, forward keys, watch for ctrl-c ──
    {
        let running   = running.clone();
        let host_stop = host.stop_flag();
        let input_tx  = host.input_sender();
        let cur_w_t   = cur_w.clone();
        let cur_h_t   = cur_h.clone();
        std::thread::spawn(move || {
            eprintln!("[veil-host] input thread started");
            let mut tick = 0u32;
            let mut btns = [false; 3];
            // Mutable local copies — updated on every Resize event so mouse
            // mapping stays correct after the terminal window is resized.
            let mut cur_cols = cols;
            let mut cur_rows = rows;
            while running.load(Ordering::Relaxed) {
                match event::poll(Duration::from_millis(100)) {
                    Ok(true)  => {}
                    Ok(false) => {
                        tick = tick.wrapping_add(1);
                        if tick % 50 == 0 { eprintln!("[veil-host] input idle ({}s)", tick / 10); }
                        continue;
                    }
                    Err(e) => { eprintln!("[veil-host] poll error: {e}"); continue; }
                }
                match event::read() {
                    Ok(ev) => {
                        eprintln!("[veil-host] event: {ev:?}");
                        match ev {
                            Event::Key(k) if is_ctrl_c(&k) => {
                                eprintln!("[veil-host] ctrl-c → shutdown");
                                running.store(false, Ordering::Relaxed);
                                host_stop.store(true, Ordering::Relaxed);
                                break;
                            }
                            Event::Key(k) => forward_key(&input_tx, k),
                            Event::Mouse(m) => {
                                let cw = cur_w_t.load(std::sync::atomic::Ordering::Relaxed);
                                let ch = cur_h_t.load(std::sync::atomic::Ordering::Relaxed);
                                forward_mouse(&input_tx, m, cur_cols, cur_rows, cw, ch, &mut btns);
                            }
                            Event::Resize(new_cols, new_rows) => {
                                cur_cols = new_cols;
                                cur_rows = new_rows;
                                let nw = new_cols as u32 * 8;
                                let nh = new_rows as u32 * 16;
                                cur_w_t.store(nw, std::sync::atomic::Ordering::Relaxed);
                                cur_h_t.store(nh, std::sync::atomic::Ordering::Relaxed);
                                let _ = input_tx.send(InputCmd::Resize { width: nw, height: nh });
                            }
                            _ => {}
                        }
                    }
                    Err(e) => eprintln!("[veil-host] read error: {e}"),
                }
            }
            eprintln!("[veil-host] input thread exited");
        });
    }

    // ── frame loop ────────────────────────────────────────────────────────────
    let mut stable_luma: Vec<u8> = Vec::new();
    while running.load(Ordering::Relaxed) {
        let mut frame = match host.frames().recv_timeout(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // Drain the channel — skip to latest frame if compositor is ahead.
        while let Ok(f) = host.frames().try_recv() { frame = f; }

        // Re-read terminal size each frame so rendering stays in sync after resize.
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let usable_rows = rows.saturating_sub(1);

        let out = match mode {
            RenderMode::Kitty => {
                let mut s = String::new();
                let _ = write!(s, "\x1b[H");
                s.push_str(&render_kitty_frame(&frame.rgba, frame.width, frame.height, cols, usable_rows));
                s
            }
            RenderMode::Halfblock => {
                let cells = rgba_to_halfblocks(&frame.rgba, frame.width, frame.height, cols, usable_rows);
                emit_halfblocks(&cells, cols, usable_rows)
            }
            RenderMode::Ascii => {
                let luma = compute_luma(&frame.rgba, frame.width, frame.height, cols, usable_rows);
                let chars = luma_to_chars(&luma, cols, usable_rows);
                emit_chars(&chars, cols, usable_rows)
            }
            RenderMode::AsciiEdge => {
                let luma = compute_luma(&frame.rgba, frame.width, frame.height, cols, usable_rows);
                if stable_luma.len() != luma.len() { stable_luma = luma.clone(); }
                apply_hysteresis(&mut stable_luma, &luma, 10);
                let chars = luma_to_chars(&stable_luma, cols, usable_rows);
                emit_chars(&chars, cols, usable_rows)
            }
        };
        stdout.write_all(out.as_bytes())?;
        stdout.flush()?;
    }

    Ok(())
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

// ─── probe ────────────────────────────────────────────────────────────────────

fn cmd_probe() -> std::io::Result<()> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let comp_w = cols as u32 * 8;
    let comp_h = rows as u32 * 16;

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
    let resolved   = vcfg.resolved_quality();
    let final_mode = quality_to_mode(resolved.clone());

    println!("terminal        : {term}");
    println!("colorterm       : {colorterm}");
    println!("term size       : {cols}x{rows} cells");
    println!("compositor      : {comp_w}x{comp_h} px  (cols×8, rows×16)");
    println!("detected quality: {:?}", detected);
    println!("config quality  : {:?}", vcfg.quality);
    println!("resolved mode   : {final_mode:?}");
    println!("fps             : {}", vcfg.fps);
    println!("config file     : {}", config_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none (using defaults)".into()));
    println!("WAYLAND_DISPLAY : {wayland}");
    println!("DISPLAY         : {display}");
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
    let f = OpenOptions::new().create(true).append(true).open(LOG_PATH)?;
    let fd = f.as_raw_fd();
    unsafe {
        // Redirect fd 2 (stderr) into the log file. We deliberately leak the
        // File so the underlying fd stays alive for the process lifetime.
        if libc::dup2(fd, 2) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    std::mem::forget(f);
    eprintln!("\n[veil-host] ── debug log opened ──");
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

// ─── terminal guard ───────────────────────────────────────────────────────────

struct TermGuard;
impl Drop for TermGuard {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(KITTY_DELETE.as_bytes());
        let _ = stdout.write_all(b"\x1b[?1006l\x1b[?1003l");
        let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

// ─── renderers ────────────────────────────────────────────────────────────────

fn emit_halfblocks(cells: &[veil_render::ColorCell], cols: u16, rows: u16) -> String {
    let mut s = String::with_capacity(cells.len() * 40);
    for row in 0..rows as usize {
        let _ = write!(s, "\x1b[{};1H", row + 1);
        for col in 0..cols as usize {
            let c = &cells[row * cols as usize + col];
            let _ = write!(
                s,
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                c.fg[0], c.fg[1], c.fg[2], c.bg[0], c.bg[1], c.bg[2]
            );
        }
    }
    s.push_str("\x1b[0m");
    s
}

fn emit_chars(chars: &[char], cols: u16, rows: u16) -> String {
    let mut s = String::with_capacity(chars.len() + rows as usize * 8);
    for row in 0..rows as usize {
        let _ = write!(s, "\x1b[{};1H", row + 1);
        for col in 0..cols as usize {
            s.push(chars[row * cols as usize + col]);
        }
    }
    s
}

// ─── mouse forwarding ─────────────────────────────────────────────────────────

fn forward_mouse(
    tx: &std::sync::mpsc::Sender<InputCmd>,
    m:  MouseEvent,
    cols: u16, rows: u16,
    comp_w: u32, comp_h: u32,
    btns: &mut [bool; 3],
) {
    // Map terminal cell coords → compositor pixel coords. We use the
    // CENTER of the cell so clicks land where users visually aim.
    let x = ((m.column as u32 * 2 + 1) * comp_w / (2 * cols.max(1) as u32)) as i32;
    let y = ((m.row    as u32 * 2 + 1) * comp_h / (2 * rows.max(1) as u32)) as i32;

    match m.kind {
        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
            let _ = tx.send(InputCmd::PointerMotionAbs { x, y, width: comp_w, height: comp_h });
        }
        MouseEventKind::Down(b) => {
            let idx = btn_idx(b);
            if btns[idx] { return; } // already down — spurious duplicate
            btns[idx] = true;
            let _ = tx.send(InputCmd::PointerMotionAbs { x, y, width: comp_w, height: comp_h });
            let _ = tx.send(InputCmd::PointerButton { button: btn_code(b), pressed: true });
        }
        MouseEventKind::Up(b) => {
            let idx = btn_idx(b);
            if !btns[idx] { return; } // already up — spurious duplicate
            btns[idx] = false;
            let _ = tx.send(InputCmd::PointerButton { button: btn_code(b), pressed: false });
        }
        MouseEventKind::ScrollDown => {
            let _ = tx.send(InputCmd::Scroll { v120:  120 });
        }
        MouseEventKind::ScrollUp => {
            let _ = tx.send(InputCmd::Scroll { v120: -120 });
        }
        _ => {}
    }
}

// evdev BTN_* codes from linux/input-event-codes.h
fn btn_code(b: MouseButton) -> u32 {
    match b {
        MouseButton::Left   => 0x110, // BTN_LEFT
        MouseButton::Right  => 0x111, // BTN_RIGHT
        MouseButton::Middle => 0x112, // BTN_MIDDLE
    }
}

fn btn_idx(b: MouseButton) -> usize {
    match b {
        MouseButton::Left   => 0,
        MouseButton::Right  => 1,
        MouseButton::Middle => 2,
    }
}

// ─── key forwarding ───────────────────────────────────────────────────────────

fn is_ctrl_c(k: &KeyEvent) -> bool {
    matches!(k.code, KeyCode::Char('c')) && k.modifiers.contains(KeyModifiers::CONTROL)
}

/// Map crossterm KeyEvent → evdev keycode, push press+release to host.
/// Lossy — the terminal already decoded our keypress into a char so we
/// reverse-map back to a keycode. Good enough for typing.
fn forward_key(tx: &std::sync::mpsc::Sender<InputCmd>, k: KeyEvent) {
    // crossterm v0.28 emits both Press and Release on terminals that
    // support the kitty keyboard protocol. On dumb terminals it only
    // emits Press. Treat unspecified as press+release pair.
    let keycode = match keycode_for(k.code) {
        Some(c) => c,
        None => return,
    };
    let shift_needed = needs_shift(k.code) || k.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl  = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt   = k.modifiers.contains(KeyModifiers::ALT);

    // Modifier holds first.
    if shift_needed { let _ = tx.send(InputCmd::Key { keycode: KEY_LEFTSHIFT, mods: 0, pressed: true }); }
    if ctrl         { let _ = tx.send(InputCmd::Key { keycode: KEY_LEFTCTRL,  mods: 0, pressed: true }); }
    if alt          { let _ = tx.send(InputCmd::Key { keycode: KEY_LEFTALT,   mods: 0, pressed: true }); }

    match k.kind {
        KeyEventKind::Release => {
            let _ = tx.send(InputCmd::Key { keycode, mods: 0, pressed: false });
        }
        _ => {
            // Press for everything else (Press / Repeat / unknown).
            let _ = tx.send(InputCmd::Key { keycode, mods: 0, pressed: true });
            let _ = tx.send(InputCmd::Key { keycode, mods: 0, pressed: false });
        }
    }

    if alt          { let _ = tx.send(InputCmd::Key { keycode: KEY_LEFTALT,   mods: 0, pressed: false }); }
    if ctrl         { let _ = tx.send(InputCmd::Key { keycode: KEY_LEFTCTRL,  mods: 0, pressed: false }); }
    if shift_needed { let _ = tx.send(InputCmd::Key { keycode: KEY_LEFTSHIFT, mods: 0, pressed: false }); }
}

fn needs_shift(code: KeyCode) -> bool {
    match code {
        KeyCode::Char(c) if c.is_ascii_uppercase() => true,
        KeyCode::Char(c) => matches!(
            c, '!'|'@'|'#'|'$'|'%'|'^'|'&'|'*'|'('|')'|'_'|'+'|'{'|'}'|'|'|':'|'"'|'<'|'>'|'?'|'~'
        ),
        _ => false,
    }
}

// evdev keycodes (linux/input-event-codes.h).
const KEY_LEFTCTRL:  u32 = 29;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_LEFTALT:   u32 = 56;

fn keycode_for(code: KeyCode) -> Option<u32> {
    Some(match code {
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            'a' => 30, 'b' => 48, 'c' => 46, 'd' => 32, 'e' => 18, 'f' => 33,
            'g' => 34, 'h' => 35, 'i' => 23, 'j' => 36, 'k' => 37, 'l' => 38,
            'm' => 50, 'n' => 49, 'o' => 24, 'p' => 25, 'q' => 16, 'r' => 19,
            's' => 31, 't' => 20, 'u' => 22, 'v' => 47, 'w' => 17, 'x' => 45,
            'y' => 21, 'z' => 44,
            '1' | '!' =>  2, '2' | '@' =>  3, '3' | '#' =>  4, '4' | '$' =>  5,
            '5' | '%' =>  6, '6' | '^' =>  7, '7' | '&' =>  8, '8' | '*' =>  9,
            '9' | '(' => 10, '0' | ')' => 11,
            '-' | '_' => 12, '=' | '+' => 13,
            '[' | '{' => 26, ']' | '}' => 27,
            '\\'| '|' => 43,
            ';' | ':' => 39, '\''| '"' => 40,
            ',' | '<' => 51, '.' | '>' => 52, '/' | '?' => 53,
            '`' | '~' => 41,
            ' ' => 57,
            _ => return None,
        },
        KeyCode::Enter     => 28,
        KeyCode::Esc       => 1,
        KeyCode::Backspace => 14,
        KeyCode::Tab       => 15,
        KeyCode::Left      => 105,
        KeyCode::Right     => 106,
        KeyCode::Up        => 103,
        KeyCode::Down      => 108,
        KeyCode::Home      => 102,
        KeyCode::End       => 107,
        KeyCode::PageUp    => 104,
        KeyCode::PageDown  => 109,
        KeyCode::Insert    => 110,
        KeyCode::Delete    => 111,
        KeyCode::F(n) => match n {
            1..=10 => 58 + n as u32,        // F1=59 .. F10=68
            11     => 87,
            12     => 88,
            _ => return None,
        },
        _ => return None,
    })
}
