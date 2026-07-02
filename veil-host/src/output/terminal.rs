//! Terminal output backend: Kitty graphics protocol, halfblock, or ASCII.
//!
//! Wraps the existing veil-render functions (rgba_to_halfblocks, compute_luma, etc.)
//! and renders to stdout via escape codes.

use std::io::{self, Write};
use std::fmt::Write as _;
use crossterm::{cursor, execute, terminal::{self, ClearType}};
use veil_render::{rgba_to_halfblocks, compute_luma, luma_to_chars, apply_hysteresis, render_kitty_frame};
use veil_gpu::GpuEncoder;

use super::OutputBackend;

#[derive(Copy, Clone, Debug)]
pub enum TerminalMode {
    Kitty,
    Halfblock,
    Ascii,
    AsciiEdge,
}

pub struct TerminalOutput {
    stdout: std::io::Stdout,
    mode: TerminalMode,
    width: u32,
    height: u32,
    cols: u16,
    rows: u16,
    gpu: Option<GpuEncoder>,
    stable_luma: Vec<u8>,
}

impl TerminalOutput {
    pub fn new() -> io::Result<Self> {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let stdout = std::io::stdout();

        // Enable raw mode and alternate screen
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout_ref = std::io::stdout();
        execute!(
            stdout_ref,
            terminal::EnterAlternateScreen,
            terminal::Clear(ClearType::All),
            cursor::Hide,
            cursor::MoveTo(0, 0),
        )?;

        // Enable mouse: only any-event + SGR modes (avoid URXVT dup events)
        stdout_ref.write_all(b"\x1b[?1003h\x1b[?1006h")?;
        stdout_ref.flush()?;

        // Detect terminal capabilities for render mode
        let mode = Self::detect_mode();

        // Try to init GPU encoder (only useful for halfblock/ascii modes)
        let gpu = if matches!(mode, TerminalMode::Kitty) {
            None
        } else {
            GpuEncoder::new()
        };

        let (pw, ph) = Self::term_pixel_size().unwrap_or((cols as u32 * 8, rows as u32 * 16));

        Ok(Self {
            stdout,
            mode,
            width: pw,
            height: ph,
            cols,
            rows,
            gpu,
            stable_luma: Vec::new(),
        })
    }

    /// Get terminal pixel dimensions from ioctl (not all terminals support this).
    fn term_pixel_size() -> Option<(u32, u32)> {
        let mut winsz: libc::winsize = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut winsz) };
        if ret == 0 && winsz.ws_xpixel > 0 && winsz.ws_ypixel > 0 {
            Some((winsz.ws_xpixel as u32, winsz.ws_ypixel as u32))
        } else {
            None
        }
    }

    /// Detect best render mode based on terminal capabilities.
    fn detect_mode() -> TerminalMode {
        let term = std::env::var("TERM").unwrap_or_default();
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();

        if term == "xterm-kitty" {
            return TerminalMode::Kitty;
        }
        if let Ok(prog) = std::env::var("TERM_PROGRAM") {
            if prog == "WezTerm" || prog.to_lowercase().contains("wezterm") {
                return TerminalMode::Kitty;
            }
        }

        if colorterm == "truecolor" || colorterm == "24bit" {
            return TerminalMode::Halfblock;
        }

        TerminalMode::Ascii
    }

    fn render_output(&mut self, rgba: &[u8]) -> String {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        self.cols = cols;
        self.rows = rows;
        let usable_rows = rows.saturating_sub(1);

        let mut out = String::new();
        let _ = write!(out, "\x1b[H");

        match self.mode {
            TerminalMode::Kitty => {
                out.push_str(&render_kitty_frame(rgba, self.width, self.height, cols, usable_rows));
            }
            TerminalMode::Halfblock => {
                let cells = if let Some(ref g) = self.gpu {
                    g.encode_halfblock(rgba, self.width, self.height, cols, usable_rows)
                } else {
                    rgba_to_halfblocks(rgba, self.width, self.height, cols, usable_rows)
                };
                Self::emit_halfblocks(&mut out, &cells, cols, usable_rows);
            }
            TerminalMode::Ascii => {
                let luma = if let Some(ref g) = self.gpu {
                    g.encode_luma(rgba, self.width, self.height, cols, usable_rows)
                } else {
                    compute_luma(rgba, self.width, self.height, cols, usable_rows)
                };
                let chars = luma_to_chars(&luma, cols, usable_rows);
                Self::emit_chars_vec(&mut out, &chars, cols, usable_rows);
            }
            TerminalMode::AsciiEdge => {
                let luma = if let Some(ref g) = self.gpu {
                    g.encode_luma(rgba, self.width, self.height, cols, usable_rows)
                } else {
                    compute_luma(rgba, self.width, self.height, cols, usable_rows)
                };
                if self.stable_luma.len() != luma.len() {
                    self.stable_luma = luma.clone();
                }
                apply_hysteresis(&mut self.stable_luma, &luma, 10);
                let chars = luma_to_chars(&self.stable_luma, cols, usable_rows);
                Self::emit_chars_vec(&mut out, &chars, cols, usable_rows);
            }
        }

        out
    }

    fn emit_halfblocks(out: &mut String, cells: &[veil_render::ColorCell], cols: u16, rows: u16) {
        for row in 0..rows as usize {
            if row > 0 {
                let _ = write!(out, "\r\n");
            }
            for col in 0..cols as usize {
                let idx = row * cols as usize + col;
                if idx < cells.len() {
                    let fg = cells[idx].fg;
                    let bg = cells[idx].bg;
                    let _ = write!(
                        out,
                        "\x1b[38;2;{};{};{};48;2;{};{};{}m▀",
                        fg[0], fg[1], fg[2],
                        bg[0], bg[1], bg[2]
                    );
                }
            }
        }
        let _ = write!(out, "\x1b[0m");
    }

    fn emit_chars_vec(out: &mut String, chars: &[char], cols: u16, rows: u16) {
        for row in 0..rows as usize {
            if row > 0 {
                let _ = write!(out, "\r\n");
            }
            for col in 0..cols as usize {
                let idx = row * cols as usize + col;
                if idx < chars.len() {
                    let _ = write!(out, "{}", chars[idx]);
                }
            }
        }
    }
}

impl OutputBackend for TerminalOutput {
    fn render_frame(&mut self, rgba: &[u8], width: u32, height: u32) -> io::Result<()> {
        self.width = width;
        self.height = height;

        let out = self.render_output(rgba);
        self.stdout.write_all(out.as_bytes())?;
        self.stdout.flush()?;

        Ok(())
    }

    fn get_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn on_vt_switch(&mut self, _switch_in: bool) -> io::Result<()> {
        // No-op for terminal output
        Ok(())
    }
}

impl Drop for TerminalOutput {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = execute!(
            self.stdout,
            cursor::Show,
            terminal::LeaveAlternateScreen,
        );
    }
}
