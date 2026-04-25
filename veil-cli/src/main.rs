use clap::{Parser, Subcommand};
use crossterm::{cursor, execute, terminal::{self, EnterAlternateScreen, LeaveAlternateScreen}};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc},
    thread,
    time::{Duration, Instant},
};
use veil_compositor::{GuiCompositor, TermCompositor};
use veil_config::load as load_config;
use veil_render::{apply_hysteresis, apply_text_overlay, luma_to_chars, render_chars};

#[derive(Parser)]
#[command(name = "veil", about = "Terminal display compositor — any app, any terminal")]
struct Cli {
    #[arg(long, default_value = "config.lua")]
    config: PathBuf,

    #[arg(long = "override", value_delimiter = ',', value_name = "KEY=VALUE")]
    overrides: Vec<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a TUI/terminal app. Ctrl+C exits.
    Run { app: String },
    /// Run a GUI app (captures with grim + Niri IPC). Ctrl+C exits.
    RunGui { app: String },
    /// Show terminal capabilities and active config
    Probe,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let mut cfg = load_config(&cli.config);

    for kv in &cli.overrides {
        if let Some((key, val)) = kv.split_once('=') {
            if key.trim() == "fps" {
                if let Ok(n) = val.trim().parse() { cfg.fps = n; }
            }
        }
    }

    match cli.command {
        Command::Run    { app } => run_tui(app, cfg),
        Command::RunGui { app } => run_gui(app, cfg),
        Command::Probe          => probe(cfg),
    }
}

// ── TUI ───────────────────────────────────────────────────────────────────────

fn run_tui(app: String, cfg: veil_config::VeilConfig) -> io::Result<()> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut compositor = TermCompositor::new();
    let mut pty_writer = compositor.launch(&app, cols, rows);
    thread::sleep(Duration::from_millis(300));

    let alive = Arc::new(AtomicBool::new(true));
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

// ── GUI ───────────────────────────────────────────────────────────────────────

fn run_gui(app: String, cfg: veil_config::VeilConfig) -> io::Result<()> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));

    // Capture at half the render FPS — grim is slow, decoupling prevents breathing.
    // The render loop just re-displays whatever the background thread last captured.
    let capture_fps = (cfg.fps / 2).max(5);

    let mut compositor = GuiCompositor::launch(
        &app, cols, rows,
        Duration::from_secs(8),
        capture_fps,
    );

    let alive = Arc::new(AtomicBool::new(true));
    let alive_ks = Arc::clone(&alive);
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut buf = [0u8; 64];
        loop {
            match stdin.lock().read(&mut buf) {
                Ok(0) | Err(_)                          => break,
                Ok(n) if buf[..n].contains(&0x03) => {
                    alive_ks.store(false, Ordering::SeqCst);
                    break;
                }
                _ => {}
            }
        }
    });

    let budget = Duration::from_secs_f64(1.0 / cfg.fps.max(1) as f64);
    let mut stdout = io::stdout();
    let _ = terminal::enable_raw_mode();
    let _ = execute!(stdout, EnterAlternateScreen, cursor::Hide);

    const HYSTERESIS: u8 = 18;

    let cell_count    = cols as usize * rows as usize;
    let mut stable    = vec![0u8; cell_count];
    let mut first     = true;

    let result = dirty_loop(
        &alive, cols, rows, budget, &mut stdout,
        || {
            if !compositor.is_running() {
                alive.store(false, Ordering::SeqCst);
            }
            let raw = compositor.capture_luma();
            if first {
                stable = raw;
                first  = false;
            } else {
                apply_hysteresis(&mut stable, &raw, HYSTERESIS);
            }
            let mut chars = luma_to_chars(&stable, cols, rows);
            let text = compositor.capture_text();
            apply_text_overlay(&mut chars, &text, cols);
            chars
        },
    );

    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
    result
}

// ── Shared render loop ────────────────────────────────────────────────────────

/// Generic dirty-row render loop. `capture` is called each tick and must return
/// exactly `cols * rows` chars. Only rows that changed since the last frame are redrawn.
fn dirty_loop(
    alive: &AtomicBool,
    cols: u16,
    rows: u16,
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
    println!("terminal : {}", std::env::var("TERM").unwrap_or_else(|_| "unknown".into()));
    println!("size     : {cols}x{rows}");
    println!("quality  : {}", cfg.quality.as_str());
    println!("fps      : {}", cfg.fps);
    Ok(())
}
