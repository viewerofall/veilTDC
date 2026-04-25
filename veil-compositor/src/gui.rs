use std::collections::HashSet;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use veil_render::{TextCell};

const ATSPI_SCRIPT: &str = include_str!("../../atspi_query.py");

pub struct GuiCompositor {
    child:       Child,
    latest_luma: Arc<Mutex<Vec<u8>>>,
    latest_text: Arc<Mutex<Vec<TextCell>>>,
    geo:         Geo,
}

impl GuiCompositor {
    pub fn launch(
        app:            &str,
        cols:           u16,
        rows:           u16,
        window_timeout: Duration,
        capture_fps:    u32,
    ) -> Self {
        let known = snapshot_niri_ids();
        eprintln!("[gui] {} existing windows before launch", known.len());

        let child = Command::new(app)
            .env("ACCESSIBILITY_ENABLED", "1")
            .env("GTK_MODULES", "gail:atk-bridge")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to launch `{app}`: {e}"));

        let child_pid = child.id();
        eprintln!("[gui] launched pid {}", child_pid);

        let (win_pid, initial_geo) = match wait_for_new_window(&known, child_pid, window_timeout) {
            Some((pid, geo)) => {
                eprintln!("[gui] window pid={} size={}×{}", pid, geo.w, geo.h);
                (Some(pid), Some(geo))
            }
            None => {
                eprintln!("[gui] no window found — falling back to full screen");
                (None, None)
            }
        };

        let latest_luma: Arc<Mutex<Vec<u8>>> =
            Arc::new(Mutex::new(vec![0u8; cols as usize * rows as usize]));
        let latest_text: Arc<Mutex<Vec<TextCell>>> =
            Arc::new(Mutex::new(Vec::new()));

        // ── Capture thread: grim window region ───────────────────────────────
        {
            let luma_ref  = Arc::clone(&latest_luma);
            let frame_dur = Duration::from_secs_f64(1.0 / capture_fps.max(1) as f64);
            let geo       = initial_geo.clone();

            thread::spawn(move || {
                loop {
                    let tick = Instant::now();

                    if let Ok(png) = grim_capture(geo.as_ref()) {
                        if let Ok(img) = image::load_from_memory_with_format(&png, image::ImageFormat::Png) {
                            let rgba = img.to_rgba8();
                            let luma = compute_luma_from_rgba(
                                rgba.as_raw(), rgba.width(), rgba.height(),
                                rgba.width() * 4, cols, rows,
                            );
                            *luma_ref.lock().unwrap() = luma;
                        }
                    }

                    if let Some(rem) = frame_dur.checked_sub(tick.elapsed()) {
                        thread::sleep(rem);
                    } else {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            });
        }

        // ── AT-SPI thread: text overlay at ~2 fps ────────────────────────────
        {
            let text_ref = Arc::clone(&latest_text);
            let pid      = win_pid;
            let geo      = initial_geo.clone();

            let script = std::env::temp_dir().join("veil_atspi.py");
            let _ = std::fs::write(&script, ATSPI_SCRIPT);

            thread::spawn(move || loop {
                if let (Some(pid), Some(ref g)) = (pid, &geo) {
                    if let Some(cells) = query_atspi(&script, pid, g, cols, rows) {
                        *text_ref.lock().unwrap() = cells;
                    }
                }
                thread::sleep(Duration::from_millis(500));
            });
        }

        thread::sleep(Duration::from_millis(300));
        Self {
            child,
            latest_luma,
            latest_text,
            geo: initial_geo.unwrap_or(Geo { x: 0, y: 0, w: 1920, h: 1080 }),
        }
    }

    pub fn capture_luma(&self) -> Vec<u8> {
        self.latest_luma.lock().unwrap().clone()
    }

    pub fn capture_text(&self) -> Vec<TextCell> {
        self.latest_text.lock().unwrap().clone()
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for GuiCompositor {
    fn drop(&mut self) { let _ = self.child.kill(); }
}

// ── Luma computation from RGBA ────────────────────────────────────────────────

fn compute_luma_from_rgba(rgba: &[u8], src_w: u32, src_h: u32, stride: u32, cols: u16, rows: u16) -> Vec<u8> {
    let dest_w = cols as u32;
    let dest_h = rows as u32;
    let dest_size = dest_w * dest_h;
    let mut luma = vec![0u8; dest_size as usize];

    for dy in 0..dest_h {
        for dx in 0..dest_w {
            let sy = (dy * src_h) / dest_h;
            let sx = (dx * src_w) / dest_w;

            if sy < src_h && sx < src_w {
                let pixel_offset = (sy * stride + sx * 4) as usize;
                if pixel_offset + 3 < rgba.len() {
                    let r = rgba[pixel_offset] as u32;
                    let g = rgba[pixel_offset + 1] as u32;
                    let b = rgba[pixel_offset + 2] as u32;
                    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) as u8;
                    luma[(dy * dest_w + dx) as usize] = y;
                }
            }
        }
    }

    luma
}

// ── AT-SPI query ──────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Geo { x: i32, y: i32, w: u32, h: u32 }

fn query_atspi(
    script: &std::path::Path,
    pid:    u32,
    geo:    &Geo,
    cols:   u16,
    rows:   u16,
) -> Option<Vec<TextCell>> {
    let out = Command::new("python3")
        .arg(script)
        .arg(pid.to_string())
        .output()
        .ok()?;

    if out.stdout.is_empty() { return None; }

    let json: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;

    let cells = json.iter().filter_map(|elem| {
        let text = elem["text"].as_str()?.trim().to_string();
        if text.is_empty() { return None; }

        let tx = elem["x"].as_i64()? as i32;
        let ty = elem["y"].as_i64()? as i32;

        let col = ((tx - geo.x) * cols as i32 / geo.w.max(1) as i32)
            .clamp(0, cols as i32 - 1) as u16;
        let row = ((ty - geo.y) * rows as i32 / geo.h.max(1) as i32)
            .clamp(0, rows as i32 - 1) as u16;

        Some(TextCell { col, row, text })
    }).collect();

    Some(cells)
}

// ── Niri IPC window detection ─────────────────────────────────────────────────

fn niri_windows() -> Option<serde_json::Value> {
    let out = Command::new("niri")
        .args(["msg", "--json", "windows"])
        .output()
        .ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

fn snapshot_niri_ids() -> HashSet<u64> {
    niri_windows()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|w| w["id"].as_u64())
        .collect()
}

fn wait_for_new_window(
    known:      &HashSet<u64>,
    child_pid:  u32,
    timeout:    Duration,
) -> Option<(u32, Geo)> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(windows) = niri_windows().and_then(|v| v.as_array().cloned()) {
            for w in &windows {
                let id  = w["id"].as_u64()?;
                let pid = w["pid"].as_u64()? as u32;

                if !known.contains(&id) || is_pid_descendant(child_pid, pid) {
                    if let Some(layout) = w.get("layout") {
                        if let Some(size) = layout["window_size"].as_array() {
                            let ww = size[0].as_u64()? as u32;
                            let wh = size[1].as_u64()? as u32;
                            if ww > 0 && wh > 0 {
                                return Some((pid, Geo { x: 0, y: 0, w: ww, h: wh }));
                            }
                        }
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    None
}

fn is_pid_descendant(parent: u32, candidate: u32) -> bool {
    if candidate == parent { return true; }
    let ppid_path = format!("/proc/{}/status", candidate);
    let Ok(text) = std::fs::read_to_string(&ppid_path) else { return false; };
    text.lines()
        .find(|l| l.starts_with("PPid:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u32>().ok())
        .map(|ppid| ppid == parent)
        .unwrap_or(false)
}

// ── grim fallback ──────────────────────────────────────────────────────────────

fn grim_capture(_geo: Option<&Geo>) -> std::io::Result<Vec<u8>> {
    Command::new("grim")
        .arg("-")
        .stdout(std::process::Stdio::piped())
        .output()
        .map(|o| o.stdout)
}
