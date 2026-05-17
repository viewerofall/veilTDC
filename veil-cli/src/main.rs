mod input;

use clap::{Parser, Subcommand};
use crossterm::{cursor, execute, terminal::{self, EnterAlternateScreen, LeaveAlternateScreen}};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc},
    thread,
    time::{Duration, Instant},
};
use veil_compositor::{GuiCompositor, TermCompositor, WaylandInput, UInputHandle, InputCmd};
use veil_config::{load as load_config, Quality};
use veil_render::{compute_luma, luma_to_chars, render_chars, render_kitty_frame, rgba_to_halfblocks, ColorCell, KITTY_DELETE};

/* ── Debug logging ───────────────────────────────────────────────────────── */

static DEBUG: AtomicBool = AtomicBool::new(false);

/// Verbose log — only emits when -d/--debug is active.
/// All eprintln! (including from crates) goes to /tmp/veil.log via stderr dup2.
macro_rules! vlog {
    ($($arg:tt)*) => {
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!($($arg)*);
        }
    }
}

fn init_debug_log() {
    use std::fs::OpenOptions;
    use std::os::unix::io::IntoRawFd;
    let file = OpenOptions::new()
        .write(true).create(true).truncate(true)
        .open("/tmp/veil.log")
        .expect("cannot open /tmp/veil.log");
    let fd = file.into_raw_fd();
    unsafe {
        libc::dup2(fd, 2); // redirect stderr → /tmp/veil.log
        libc::close(fd);
    }
    eprintln!("=== veil debug log ===");
}

/* ── CLI ─────────────────────────────────────────────────────────────────── */

#[derive(Parser)]
#[command(name = "veil", about = "Terminal display compositor — any app, any terminal")]
struct Cli {
    /// Path to config file (default: ~/.config/veil-config.lua)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Override config values, e.g. --override fps=60,quality=pixel
    #[arg(long = "override", value_delimiter = ',', value_name = "KEY=VALUE")]
    overrides: Vec<String>,

    /// Write verbose debug log to /tmp/veil.log
    #[arg(short = 'd', long)]
    debug: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a TUI/terminal app. Ctrl+C exits.
    Run { app: String },
    /// Run a GUI app captured via cage compositor. Ctrl+C exits.
    RunGui { app: String },
    /// Show terminal capabilities and resolved config.
    Probe,
    /// Send a test kitty graphics frame directly (no cage, no alt-screen).
    TestKitty,
}

fn default_config_path() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".config/veil-config.lua"))
        .unwrap_or_else(|_| PathBuf::from("veil-config.lua"))
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    if cli.debug {
        DEBUG.store(true, Ordering::Relaxed);
        init_debug_log();
        eprintln!("[veil] debug mode — full log at /tmp/veil.log");
    }

    let config_path = cli.config.unwrap_or_else(default_config_path);
    let mut cfg     = load_config(&config_path);

    for kv in &cli.overrides {
        if let Some((key, val)) = kv.split_once('=') {
            match key.trim() {
                "fps"               => { if let Ok(n) = val.trim().parse() { cfg.fps = n; } }
                "quality"           => { cfg.quality = Quality::from_str(val.trim()); }
                "cage_timeout_secs" => { if let Ok(n) = val.trim().parse() { cfg.cage_timeout_secs = n; } }
                "input"             => { cfg.input = !matches!(val.trim(), "false" | "0" | "off"); }
                _ => {}
            }
        }
    }

    vlog!("[veil] config: quality={} fps={} cage_timeout={}s input={}",
        cfg.quality.as_str(), cfg.fps, cfg.cage_timeout_secs, cfg.input);

    match cli.command {
        Command::Run       { app } => run_tui(app, cfg),
        Command::RunGui    { app } => run_gui(app, cfg),
        Command::Probe             => probe(cfg),
        Command::TestKitty         => test_kitty(),
    }
}

// ── TUI ───────────────────────────────────────────────────────────────────────

fn run_tui(app: String, cfg: veil_config::VeilConfig) -> io::Result<()> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut compositor = TermCompositor::new();
    let mut pty_writer = compositor.launch(&app, cols, rows);
    thread::sleep(Duration::from_millis(300));

    let alive    = Arc::new(AtomicBool::new(true));
    let alive_in = Arc::clone(&alive);
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut buf = [0u8; 64];
        loop {
            match stdin.lock().read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf[..n].contains(&0x03) {
                        alive_in.store(false, Ordering::SeqCst);
                        break;
                    }
                    let _ = pty_writer.write_all(&buf[..n]);
                    let _ = pty_writer.flush();
                }
            }
        }
    });

    let budget = Duration::from_secs_f64(1.0 / cfg.fps.max(1) as f64);
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

    let result = dirty_loop(
        &alive, cols, rows, budget, &mut stdout,
        || render_chars(&compositor.capture()),
    );

    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
    result
}

// ── Input abstraction ─────────────────────────────────────────────────────────

enum AnyInput {
    Wayland(WaylandInput),
    UInput(UInputHandle),
}

impl AnyInput {
    fn send(&self, cmd: InputCmd) {
        match self {
            AnyInput::Wayland(w) => w.send(cmd),
            AnyInput::UInput(u)  => u.send(cmd),
        }
    }
}

// ── GUI ───────────────────────────────────────────────────────────────────────

fn run_gui(app: String, cfg: veil_config::VeilConfig) -> io::Result<()> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let capture_fps  = (cfg.fps / 2).max(5);
    let quality      = cfg.resolved_quality();

    eprintln!("[veil] quality={} fps={} cage_timeout={}s input={}",
        quality.as_str(), cfg.fps, cfg.cage_timeout_secs, cfg.input);

    let mut compositor = GuiCompositor::launch(
        &app, cols, rows,
        Duration::from_secs(cfg.cage_timeout_secs as u64),
        capture_fps,
    );

    let any_input: Option<AnyInput> = if cfg.input {
        if let Some(wi) = compositor.cage_socket.as_deref().and_then(WaylandInput::connect_to_socket) {
            eprintln!("[input] using Wayland virtual keyboard/pointer");
            Some(AnyInput::Wayland(wi))
        } else if let Some(ui) = UInputHandle::new() {
            eprintln!("[input] using uinput virtual device");
            Some(AnyInput::UInput(ui))
        } else {
            eprintln!("[input] all input methods failed — read-only mode");
            None
        }
    } else {
        eprintln!("[input] disabled by config");
        None
    };

    let input_rx   = input::spawn_input_thread();
    let alive      = Arc::new(AtomicBool::new(true));
    let budget     = Duration::from_secs_f64(1.0 / cfg.fps.max(1) as f64);
    let mut stdout = io::stdout();
    let _ = terminal::enable_raw_mode();
    let _ = execute!(stdout, EnterAlternateScreen, cursor::Hide);

    let result = match quality {
        Quality::AsciiLuma | Quality::AsciiEdge => {
            let edge = quality == Quality::AsciiEdge;
            ascii_gui_loop(&alive, cols, rows, budget, &mut stdout,
                input_rx, any_input, &mut compositor, edge)
        }
        Quality::Kitty => {
            kitty_gui_loop(&alive, cols, rows, budget, &mut stdout,
                input_rx, any_input, &mut compositor)
        }
        Quality::Sixel => {
            eprintln!("[render] sixel not yet implemented, falling back to pixel");
            pixel_gui_loop(&alive, cols, rows, budget, &mut stdout,
                input_rx, any_input, &mut compositor)
        }
        _ => pixel_gui_loop(&alive, cols, rows, budget, &mut stdout,
                input_rx, any_input, &mut compositor),
    };

    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
    result
}

// ── Pixel GUI loop (color halfblocks) ────────────────────────────────────────

fn pixel_gui_loop(
    alive:         &AtomicBool,
    mut cols:      u16,
    mut rows:      u16,
    budget:        Duration,
    stdout:        &mut io::Stdout,
    input_rx:      std::sync::mpsc::Receiver<input::InputEvent>,
    any_input: Option<AnyInput>,
    compositor:    &mut GuiCompositor,
) -> io::Result<()> {
    let make_black = || ColorCell { fg: [0, 0, 0], bg: [0, 0, 0] };
    let mut prev = vec![make_black(); cols as usize * rows as usize];

    while alive.load(Ordering::SeqCst) {
        let tick = Instant::now();

        if let Some((w, h)) = drain_input(&input_rx, alive, &any_input, cols, rows) {
            cols = w; rows = h;
            prev = vec![make_black(); cols as usize * rows as usize];
            execute!(stdout, terminal::Clear(terminal::ClearType::All))?;
        }
        if !alive.load(Ordering::SeqCst) { break; }
        if !compositor.is_running() { break; }

        let (w, h, rgba) = compositor.capture_rgba();
        vlog!("[pixel] capture: {}x{} {} bytes", w, h, rgba.len());
        let curr = if w == 0 || h == 0 || rgba.is_empty() {
            vec![make_black(); cols as usize * rows as usize]
        } else {
            rgba_to_halfblocks(&rgba, w, h, cols, rows)
        };

        for row in 0..rows {
            let s = row as usize * cols as usize;
            let e = (s + cols as usize).min(curr.len());
            if e <= s || curr[s..e] == prev[s..e] { continue; }
            execute!(stdout, cursor::MoveTo(0, row))?;
            let mut line = String::with_capacity(cols as usize * 32);
            for cell in &curr[s..e] {
                use std::fmt::Write as _;
                let _ = write!(
                    line,
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m\u{2580}",
                    cell.fg[0], cell.fg[1], cell.fg[2],
                    cell.bg[0], cell.bg[1], cell.bg[2],
                );
            }
            line.push_str("\x1b[0m");
            write!(stdout, "{line}")?;
        }
        stdout.flush()?;
        prev = curr;

        if let Some(rem) = budget.checked_sub(tick.elapsed()) {
            thread::sleep(rem);
        }
    }
    Ok(())
}

// ── ASCII GUI loop (luma chars) ───────────────────────────────────────────────

fn ascii_gui_loop(
    alive:         &AtomicBool,
    mut cols:      u16,
    mut rows:      u16,
    budget:        Duration,
    stdout:        &mut io::Stdout,
    input_rx:      std::sync::mpsc::Receiver<input::InputEvent>,
    any_input: Option<AnyInput>,
    compositor:    &mut GuiCompositor,
    edge:          bool,
) -> io::Result<()> {
    let mut prev = vec![' '; cols as usize * rows as usize];

    while alive.load(Ordering::SeqCst) {
        let tick = Instant::now();

        if let Some((w, h)) = drain_input(&input_rx, alive, &any_input, cols, rows) {
            cols = w; rows = h;
            prev = vec![' '; cols as usize * rows as usize];
            execute!(stdout, terminal::Clear(terminal::ClearType::All))?;
        }
        if !alive.load(Ordering::SeqCst) { break; }
        if !compositor.is_running() { break; }

        let (w, h, rgba) = compositor.capture_rgba();
        let curr = if w == 0 || h == 0 || rgba.is_empty() {
            vec![' '; cols as usize * rows as usize]
        } else {
            let luma = compute_luma(&rgba, w, h, cols, rows);
            if edge {
                luma_to_chars(&luma, cols, rows)
            } else {
                luma.iter().map(|&l| veil_render::luma_to_char(l)).collect()
            }
        };

        for row in 0..rows {
            let s = row as usize * cols as usize;
            let e = (s + cols as usize).min(curr.len());
            if e <= s || curr[s..e] == prev[s..e] { continue; }
            execute!(stdout, cursor::MoveTo(0, row))?;
            write!(stdout, "{}", curr[s..e].iter().collect::<String>())?;
        }
        stdout.flush()?;
        prev = curr;

        if let Some(rem) = budget.checked_sub(tick.elapsed()) {
            thread::sleep(rem);
        }
    }
    Ok(())
}

// ── Kitty graphics protocol loop ─────────────────────────────────────────────

fn kitty_gui_loop(
    alive:         &AtomicBool,
    mut cols:      u16,
    mut rows:      u16,
    budget:        Duration,
    stdout:        &mut io::Stdout,
    input_rx:      std::sync::mpsc::Receiver<input::InputEvent>,
    any_input: Option<AnyInput>,
    compositor:    &mut GuiCompositor,
) -> io::Result<()> {
    let mut frame_count: u64 = 0;
    let mut empty_count: u64 = 0;

    while alive.load(Ordering::SeqCst) {
        let tick = Instant::now();

        if let Some((w, h)) = drain_input(&input_rx, alive, &any_input, cols, rows) {
            cols = w; rows = h;
            vlog!("[kitty] resize → {}x{}", cols, rows);
            write!(stdout, "{KITTY_DELETE}")?;
            execute!(stdout, terminal::Clear(terminal::ClearType::All))?;
        }
        if !alive.load(Ordering::SeqCst) { break; }
        if !compositor.is_running() {
            vlog!("[kitty] compositor exited — leaving loop");
            break;
        }

        let (w, h, rgba) = compositor.capture_rgba();
        vlog!("[kitty] capture: {}x{} {} bytes", w, h, rgba.len());

        let frame = render_kitty_frame(&rgba, w, h, cols, rows);
        if !frame.is_empty() {
            frame_count += 1;
            vlog!("[kitty] frame #{} payload={} bytes", frame_count, frame.len());
            execute!(stdout, cursor::MoveTo(0, 0))?;
            write!(stdout, "{frame}")?;
            stdout.flush()?;
        } else {
            empty_count += 1;
            if empty_count <= 5 || empty_count % 60 == 0 {
                vlog!("[kitty] empty frame #{} (no data yet)", empty_count);
            }
        }

        if let Some(rem) = budget.checked_sub(tick.elapsed()) {
            thread::sleep(rem);
        }
    }

    vlog!("[kitty] loop ended: {} frames rendered, {} empty", frame_count, empty_count);
    write!(stdout, "{KITTY_DELETE}")?;
    stdout.flush()?;
    Ok(())
}

// ── Shared input drain ────────────────────────────────────────────────────────

/// Drain all pending input events. Returns Some((cols, rows)) if terminal was resized.
/// Stores false into `alive` on Ctrl+C.
fn drain_input(
    input_rx:      &std::sync::mpsc::Receiver<input::InputEvent>,
    alive:         &AtomicBool,
    any_input: &Option<AnyInput>,
    cols:          u16,
    rows:          u16,
) -> Option<(u16, u16)> {
    let mut resize = None;
    while let Ok(ev) = input_rx.try_recv() {
        if let input::InputEvent::Key(k) = &ev {
            use crossterm::event::{KeyCode, KeyModifiers};
            if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
                alive.store(false, Ordering::SeqCst);
                return resize;
            }
        }
        if let Some(ai) = any_input {
            if let Some(sz) = input::forward_event(&ev, &|cmd| ai.send(cmd), cols, rows) {
                resize = Some(sz);
            }
        } else if let input::InputEvent::Resize(w, h) = ev {
            resize = Some((w, h));
        }
    }
    resize
}

// ── TUI render loop ───────────────────────────────────────────────────────────

fn dirty_loop(
    alive:  &AtomicBool,
    cols:   u16,
    rows:   u16,
    budget: Duration,
    stdout: &mut io::Stdout,
    mut capture: impl FnMut() -> Vec<char>,
) -> io::Result<()> {
    let mut prev = vec![' '; cols as usize * rows as usize];

    while alive.load(Ordering::SeqCst) {
        let tick = Instant::now();
        let curr = capture();

        for row in 0..rows {
            let s = row as usize * cols as usize;
            let e = (s + cols as usize).min(curr.len());
            if e <= s { continue; }
            if curr[s..e] != prev[s..e] {
                execute!(stdout, cursor::MoveTo(0, row))?;
                write!(stdout, "{}", curr[s..e].iter().collect::<String>())?;
            }
        }
        stdout.flush()?;
        prev = curr;

        if let Some(rem) = budget.checked_sub(tick.elapsed()) {
            thread::sleep(rem);
        }
    }
    Ok(())
}

fn probe(cfg: veil_config::VeilConfig) -> io::Result<()> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let resolved     = cfg.resolved_quality();
    println!("terminal     : {}", std::env::var("TERM").unwrap_or_else(|_| "unknown".into()));
    println!("colorterm    : {}", std::env::var("COLORTERM").unwrap_or_else(|_| "unknown".into()));
    println!("size         : {cols}x{rows}");
    println!("quality      : {} (resolved: {})", cfg.quality.as_str(), resolved.as_str());
    println!("fps          : {}", cfg.fps);
    println!("cage_timeout : {}s", cfg.cage_timeout_secs);
    println!("input        : {}", cfg.input);
    Ok(())
}

fn test_kitty() -> io::Result<()> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    eprintln!("[test-kitty] terminal size: {}x{}", cols, rows);

    // 200x100 RGBA test pattern: four quadrants R/G/B/W
    let iw: u32 = 200;
    let ih: u32 = 100;
    let mut rgba = vec![0u8; (iw * ih * 4) as usize];
    for y in 0..ih {
        for x in 0..iw {
            let off = ((y * iw + x) * 4) as usize;
            let (r, g, b) = match (x < iw / 2, y < ih / 2) {
                (true,  true)  => (220,  50,  50),  // red
                (false, true)  => ( 50, 200,  50),  // green
                (true,  false) => ( 50,  50, 220),  // blue
                (false, false) => (200, 200, 200),  // white
            };
            rgba[off]     = r;
            rgba[off + 1] = g;
            rgba[off + 2] = b;
            rgba[off + 3] = 255;
        }
    }

    // Use half the terminal so text is visible before/after
    let show_cols = cols;
    let show_rows = rows / 2;

    let frame = render_kitty_frame(&rgba, iw, ih, show_cols, show_rows);

    let mut stdout = io::stdout();
    writeln!(stdout, "=== veil test-kitty: image below, press Enter to exit ===")?;
    write!(stdout, "{frame}")?;
    // Move cursor below image area and print a marker
    execute!(stdout, cursor::MoveTo(0, show_rows + 2))?;
    writeln!(stdout, "=== end of image area ===")?;
    stdout.flush()?;

    // Wait for Enter
    let mut buf = [0u8; 1];
    io::stdin().read(&mut buf).ok();
    Ok(())
}
