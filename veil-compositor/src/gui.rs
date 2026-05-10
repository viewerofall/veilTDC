use std::collections::HashSet;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use veil_render::TextCell;
use crate::wayland_capture::WaylandCapture;

const ATSPI_SCRIPT: &str = include_str!("../../atspi_query.py");

/* ── Compositor detection ───────────────────────────────────────────────── */

#[derive(Clone, Copy, PartialEq)]
enum CompositorKind { Niri, Hyprland, Unknown }

fn detect_compositor() -> CompositorKind {
    if std::env::var("NIRI_SOCKET").is_ok() {
        return CompositorKind::Niri;
    }
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        return CompositorKind::Hyprland;
    }
    CompositorKind::Unknown
}

/* ── Window info ─────────────────────────────────────────────────────────── */

#[derive(Clone, Debug)]
struct WindowInfo {
    pid:    u32,
    app_id: String,
    title:  String,
    x:      i32,
    y:      i32,
    w:      u32,
    h:      u32,
}

fn list_niri_windows() -> Vec<WindowInfo> {
    (|| -> Option<Vec<WindowInfo>> {
        let out = Command::new("niri").args(["msg", "--json", "windows"]).output().ok()?;
        let json: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
        Some(
            json.iter()
                .filter_map(|w| {
                    let pid    = w["pid"].as_u64()? as u32;
                    let app_id = w["app_id"].as_str().unwrap_or("").to_string();
                    let title  = w["title"].as_str().unwrap_or("").to_string();

                    // Extract window size from layout
                    let layout = &w["layout"];
                    let window_size = layout["window_size"].as_array()?;
                    let ww = window_size[0].as_i64()? as u32;
                    let wh = window_size[1].as_i64()? as u32;

                    if ww == 0 || wh == 0 { return None; }

                    // For position: try to use tile_size and tile_pos, or default to 0,0
                    // (Niri doesn't expose absolute screen coordinates easily)
                    let tile_size = layout["tile_size"].as_array();
                    let tile_pos = layout["tile_pos_in_workspace_view"].as_array();
                    let offset = layout["window_offset_in_tile"].as_array();

                    let (x, y) = if let (Some(ts), Some(tp), Some(off)) = (tile_size, tile_pos, offset) {
                        let tile_w = ts[0].as_f64().unwrap_or(0.0) as i32;
                        let tile_h = ts[1].as_f64().unwrap_or(0.0) as i32;
                        let col = tp[0].as_i64().unwrap_or(0) as i32;
                        let row = tp[1].as_i64().unwrap_or(0) as i32;
                        let off_x = off[0].as_f64().unwrap_or(0.0) as i32;
                        let off_y = off[1].as_f64().unwrap_or(0.0) as i32;
                        (col * tile_w + off_x, row * tile_h + off_y)
                    } else {
                        (0, 0)
                    };

                    eprintln!("[window] {} (pid={}) size={}x{} pos={},{}", app_id, pid, ww, wh, x, y);
                    Some(WindowInfo { pid, app_id, title, x, y, w: ww, h: wh })
                })
                .collect(),
        )
    })()
    .unwrap_or_default()
}

fn list_hyprland_windows() -> Vec<WindowInfo> {
    (|| -> Option<Vec<WindowInfo>> {
        let out = Command::new("hyprctl").args(["clients", "-j"]).output().ok()?;
        let json: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
        Some(
            json.iter()
                .filter_map(|w| {
                    if w["mapped"].as_bool() == Some(false) { return None; }
                    let pid    = w["pid"].as_u64()? as u32;
                    let app_id = w["class"].as_str().unwrap_or("").to_string();
                    let title  = w["title"].as_str().unwrap_or("").to_string();
                    let at     = w["at"].as_array()?;
                    let size   = w["size"].as_array()?;
                    let x  = at[0].as_i64().unwrap_or(0) as i32;
                    let y  = at[1].as_i64().unwrap_or(0) as i32;
                    let ww = size[0].as_u64()? as u32;
                    let wh = size[1].as_u64()? as u32;
                    if ww == 0 || wh == 0 { return None; }
                    Some(WindowInfo { pid, app_id, title, x, y, w: ww, h: wh })
                })
                .collect(),
        )
    })()
    .unwrap_or_default()
}

fn wayland_sockets() -> HashSet<String> {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());
    std::fs::read_dir(&dir).ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("wayland-")
                && e.metadata().ok().map(|m| m.file_type().is_socket()).unwrap_or(false)
            { Some(name) } else { None }
        })
        .collect()
}

fn wait_for_new_socket(existing: &HashSet<String>, timeout: Duration) -> Option<String> {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("wayland-") && !existing.contains(&name) {
                    if entry.metadata().ok().map(|m| m.file_type().is_socket()).unwrap_or(false) {
                        return Some(name);
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    None
}

fn list_windows(kind: CompositorKind) -> Vec<WindowInfo> {
    match kind {
        CompositorKind::Niri     => list_niri_windows(),
        CompositorKind::Hyprland => list_hyprland_windows(),
        CompositorKind::Unknown  => Vec::new(),
    }
}

fn snapshot_pids(kind: CompositorKind) -> HashSet<u32> {
    list_windows(kind).into_iter().map(|w| w.pid).collect()
}

fn wait_for_new_window(
    known:      &HashSet<u32>,
    child_pid:  u32,
    kind:       CompositorKind,
    timeout:    Duration,
) -> Option<WindowInfo> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        for w in list_windows(kind) {
            if !known.contains(&w.pid) || is_pid_descendant(child_pid, w.pid) {
                if w.w > 0 && w.h > 0 {
                    return Some(w);
                }
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    None
}

fn is_pid_descendant(parent: u32, candidate: u32) -> bool {
    if candidate == parent { return true; }
    let path = format!("/proc/{}/status", candidate);
    let Ok(text) = std::fs::read_to_string(path) else { return false; };
    text.lines()
        .find(|l| l.starts_with("PPid:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u32>().ok())
        .map(|ppid| ppid == parent)
        .unwrap_or(false)
}

/* ── Sibling binary discovery ───────────────────────────────────────────── */

fn find_sibling(name: &str) -> Option<PathBuf> {
    // 1. Next to current exe — covers both dev (target/release/) and installed (/usr/local/bin/)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c = dir.join(name);
            if c.exists() { return Some(c); }
        }
    }
    // 2. System lib dir for .so files (installed by `make install`)
    let sys = PathBuf::from("/usr/local/lib/veil").join(name);
    if sys.exists() { return Some(sys); }

    // 3. User lib dir
    if let Ok(home) = std::env::var("HOME") {
        let user = PathBuf::from(home).join(".local/lib/veil").join(name);
        if user.exists() { return Some(user); }
    }
    None
}

/* ── RGBA crop ──────────────────────────────────────────────────────────── */

/// Crop a physical-pixel RGBA buffer to a logical-pixel window region.
/// Returns (cropped_w, cropped_h, rgba) in physical pixels.
fn crop_rgba(src: &[u8], src_w: u32, src_h: u32,
             x: i32, y: i32, w: u32, h: u32, scale: i32) -> (u32, u32, Vec<u8>) {
    let s  = scale.max(1) as u32;
    let px = (x.max(0) as u32) * s;
    let py = (y.max(0) as u32) * s;
    let pw = (w * s).min(src_w.saturating_sub(px));
    let ph = (h * s).min(src_h.saturating_sub(py));
    if pw == 0 || ph == 0 { return (0, 0, Vec::new()); }
    let mut out = Vec::with_capacity((pw * ph * 4) as usize);
    for row in py..(py + ph) {
        let s_off = (row * src_w + px) as usize * 4;
        let e_off = s_off + pw as usize * 4;
        if e_off <= src.len() { out.extend_from_slice(&src[s_off..e_off]); }
    }
    (pw, ph, out)
}

/* ── AT-SPI query ───────────────────────────────────────────────────────── */

#[derive(Clone)]
struct Geo { x: i32, y: i32, w: u32, h: u32 }

fn query_atspi(script: &std::path::Path, pid: u32, geo: &Geo, cols: u16, rows: u16) -> Option<Vec<TextCell>> {
    let out = Command::new("python3").arg(script).arg(pid.to_string()).output().ok()?;
    if out.stdout.is_empty() { return None; }
    let json: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    let cells = json.iter().filter_map(|e| {
        let text = e["text"].as_str()?.trim().to_string();
        if text.is_empty() { return None; }
        let tx = e["x"].as_i64()? as i32;
        let ty = e["y"].as_i64()? as i32;
        let col = ((tx - geo.x) * cols as i32 / geo.w.max(1) as i32).clamp(0, cols as i32 - 1) as u16;
        let row = ((ty - geo.y) * rows as i32 / geo.h.max(1) as i32).clamp(0, rows as i32 - 1) as u16;
        Some(TextCell { col, row, text })
    }).collect();
    Some(cells)
}

/* ── GuiCompositor ──────────────────────────────────────────────────────── */

pub struct GuiCompositor {
    child:       Child,
    latest_rgba: Arc<Mutex<(u32, u32, Vec<u8>)>>,
    latest_text: Arc<Mutex<Vec<TextCell>>>,
}

impl GuiCompositor {
    pub fn launch(
        app:            &str,
        _cols:          u16,
        _rows:          u16,
        _window_timeout: Duration,
        capture_fps:    u32,
    ) -> Self {
        // Snapshot existing wayland sockets so we can detect cage's new one.
        let existing_sockets = wayland_sockets();
        eprintln!("[gui] existing sockets: {:?}", existing_sockets);

        // Launch app inside cage (isolated Wayland compositor).
        // cage creates its own zwlr_screencopy_manager_v1 — only the app is visible.
        let child = Command::new("cage")
            .arg("--")
            .arg(app)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to launch cage: {e}"));
        let cage_pid = child.id();
        eprintln!("[gui] cage pid={}", cage_pid);

        let latest_rgba: Arc<Mutex<(u32, u32, Vec<u8>)>> =
            Arc::new(Mutex::new((0, 0, Vec::new())));
        let latest_text: Arc<Mutex<Vec<TextCell>>> = Arc::new(Mutex::new(Vec::new()));

        // ── Capture thread ────────────────────────────────────────────────
        {
            let rgba_ref  = Arc::clone(&latest_rgba);
            let frame_dur = Duration::from_secs_f64(1.0 / capture_fps.max(1) as f64);

            thread::spawn(move || {
                // Wait for cage's Wayland socket to appear
                let runtime_dir =
                    std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());

                let cage_socket = match wait_for_new_socket(&existing_sockets, Duration::from_secs(10)) {
                    Some(s) => s,
                    None => {
                        eprintln!("[capture] cage socket never appeared — idling");
                        loop { thread::sleep(Duration::from_millis(500)); }
                    }
                };

                let socket_path = format!("{}/{}", runtime_dir, cage_socket);
                eprintln!("[capture] cage socket: {}", socket_path);

                // Give cage a moment to finish initializing its compositor
                thread::sleep(Duration::from_millis(300));

                match WaylandCapture::connect_to_socket(&socket_path) {
                    Some(mut wc) => {
                        eprintln!("[capture] screencopy connected to cage");
                        loop {
                            let tick = Instant::now();
                            if let Some((w, h, rgba)) = wc.capture_full() {
                                *rgba_ref.lock().unwrap() = (w, h, rgba);
                            }
                            if let Some(rem) = frame_dur.checked_sub(tick.elapsed()) {
                                thread::sleep(rem);
                            } else {
                                thread::sleep(Duration::from_millis(8));
                            }
                        }
                    }
                    None => {
                        eprintln!("[capture] could not connect screencopy to cage — idling");
                        loop { thread::sleep(Duration::from_millis(500)); }
                    }
                }
            });
        }

        thread::sleep(Duration::from_millis(200));
        Self { child, latest_rgba, latest_text }
    }

    pub fn capture_rgba(&self) -> (u32, u32, Vec<u8>) {
        self.latest_rgba.lock().unwrap().clone()
    }

    pub fn capture_text(&self) -> Vec<TextCell> {
        self.latest_text.lock().unwrap().clone()
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for GuiCompositor {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn compositor_name(k: CompositorKind) -> &'static str {
    match k {
        CompositorKind::Niri     => "Niri",
        CompositorKind::Hyprland => "Hyprland",
        CompositorKind::Unknown  => "unknown",
    }
}
