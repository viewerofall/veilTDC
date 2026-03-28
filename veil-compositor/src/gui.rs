use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use veil_render::{compute_luma, TextCell};

// Embedded AT-SPI scraper — written to a temp file at runtime.
const ATSPI_SCRIPT: &str = include_str!("../../atspi_query.py");

pub struct GuiCompositor {
    child:       Child,
    latest_luma: Arc<Mutex<Vec<u8>>>,
    latest_text: Arc<Mutex<Vec<TextCell>>>,
}

impl GuiCompositor {
    pub fn launch(
        app:            &str,
        cols:           u16,
        rows:           u16,
        window_timeout: Duration,
        capture_fps:    u32,
    ) -> Self {
        let known = snapshot_addresses();
        eprintln!("[gui] {} existing windows before launch", known.len());

        // Enable AT-SPI in the launched app
        let child = Command::new(app)
            .env("ACCESSIBILITY_ENABLED", "1")
            .env("GTK_MODULES", "gail:atk-bridge")
            .spawn()
            .unwrap_or_else(|e| panic!("failed to launch `{app}`: {e}"));

        eprintln!("[gui] launched pid {}", child.id());

        let (win_addr, win_pid, initial_geo) = match wait_for_new_window(&known, window_timeout) {
            Some(info) => {
                eprintln!("[gui] window {} (pid {}) at {},{} {}×{}",
                    info.addr, info.pid, info.geo.x, info.geo.y, info.geo.w, info.geo.h);
                let g = info.geo;
                (Some(info.addr), Some(info.pid), Some(g))
            }
            None => {
                eprintln!("[gui] no window found — falling back to full screen");
                (None, None, None)
            }
        };

        let latest_luma: Arc<Mutex<Vec<u8>>> =
            Arc::new(Mutex::new(vec![0u8; cols as usize * rows as usize]));
        let latest_text: Arc<Mutex<Vec<TextCell>>> =
            Arc::new(Mutex::new(Vec::new()));

        // ── Capture thread: grim at capture_fps ──────────────────────────────
        {
            let luma_ref   = Arc::clone(&latest_luma);
            let addr       = win_addr.clone();
            let frame_dur  = Duration::from_secs_f64(1.0 / capture_fps.max(1) as f64);

            thread::spawn(move || loop {
                let tick = Instant::now();

                let png = match addr.as_deref().and_then(query_window_geo) {
                    Some(g) => grim_region(g.x, g.y, g.w, g.h),
                    None    => grim_fullscreen(),
                };

                if !png.is_empty() {
                    if let Ok(img) = image::load_from_memory_with_format(
                        &png, image::ImageFormat::Png,
                    ) {
                        let rgba = img.to_rgba8();
                        let luma = compute_luma(
                            rgba.as_raw(), rgba.width(), rgba.height(), cols, rows,
                        );
                        *luma_ref.lock().unwrap() = luma;
                    }
                }

                if let Some(rem) = frame_dur.checked_sub(tick.elapsed()) {
                    thread::sleep(rem);
                }
            });
        }

        // ── AT-SPI thread: text overlay at ~2 fps ────────────────────────────
        {
            let text_ref = Arc::clone(&latest_text);
            let pid      = win_pid;
            let geo      = initial_geo;

            // Write the embedded Python script to a temp file once
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
        Self { child, latest_luma, latest_text }
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

// ── AT-SPI query ──────────────────────────────────────────────────────────────

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

        // Map from screen pixel coords to terminal cell coords
        let col = ((tx - geo.x) * cols as i32 / geo.w.max(1) as i32)
            .clamp(0, cols as i32 - 1) as u16;
        let row = ((ty - geo.y) * rows as i32 / geo.h.max(1) as i32)
            .clamp(0, rows as i32 - 1) as u16;

        Some(TextCell { col, row, text })
    }).collect();

    Some(cells)
}

// ── window detection ──────────────────────────────────────────────────────────

struct Geo { x: i32, y: i32, w: u32, h: u32 }

struct WindowInfo {
    addr: String,
    pid:  u32,
    geo:  Geo,
}

fn snapshot_addresses() -> HashSet<String> {
    let Ok(out) = Command::new("hyprctl").args(["clients", "-j"]).output() else {
        return HashSet::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return HashSet::new();
    };
    json.as_array()
        .map(|a| a.iter().filter_map(|c| c["address"].as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn wait_for_new_window(known: &HashSet<String>, timeout: Duration) -> Option<WindowInfo> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let Ok(out) = Command::new("hyprctl").args(["clients", "-j"]).output() else {
            thread::sleep(Duration::from_millis(150)); continue;
        };
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
            thread::sleep(Duration::from_millis(150)); continue;
        };
        if let Some(clients) = json.as_array() {
            for client in clients {
                let addr = client["address"].as_str().unwrap_or("");
                if known.contains(addr) { continue; }
                if let Some(geo) = extract_geo(client) {
                    let pid = client["pid"].as_u64().unwrap_or(0) as u32;
                    return Some(WindowInfo { addr: addr.to_string(), pid, geo });
                }
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    None
}

fn query_window_geo(address: &str) -> Option<Geo> {
    let out = Command::new("hyprctl").args(["clients", "-j"]).output().ok()?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    json.as_array()?
        .iter()
        .find(|c| c["address"].as_str() == Some(address))
        .and_then(|c| extract_geo(c))
}

fn extract_geo(c: &serde_json::Value) -> Option<Geo> {
    let at   = c["at"].as_array()?;
    let size = c["size"].as_array()?;
    let w = size[0].as_i64()? as u32;
    let h = size[1].as_i64()? as u32;
    if w == 0 || h == 0 { return None; }
    Some(Geo { x: at[0].as_i64()? as i32, y: at[1].as_i64()? as i32, w, h })
}

fn grim_region(x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
    Command::new("grim")
        .args(["-g", &format!("{x},{y} {w}x{h}"), "-"])
        .stdout(Stdio::piped()).output().map(|o| o.stdout).unwrap_or_default()
}

fn grim_fullscreen() -> Vec<u8> {
    Command::new("grim").args(["-"]).stdout(Stdio::piped())
        .output().map(|o| o.stdout).unwrap_or_default()
}
