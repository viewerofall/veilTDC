use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use veil_render::TextCell;
use crate::capture_shm::ShmCapture;
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
                    // "geometry" gives logical-pixel position + size on the output
                    let geo = &w["geometry"];
                    let x  = geo["x"].as_i64().unwrap_or(0) as i32;
                    let y  = geo["y"].as_i64().unwrap_or(0) as i32;
                    let ww = geo["width"].as_u64().unwrap_or(0) as u32;
                    let wh = geo["height"].as_u64().unwrap_or(0) as u32;
                    if ww == 0 || wh == 0 { return None; }
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
        cols:           u16,
        rows:           u16,
        window_timeout: Duration,
        capture_fps:    u32,
    ) -> Self {
        let compositor = detect_compositor();
        let known      = snapshot_pids(compositor);
        eprintln!("[gui] compositor={:?} existing_pids={}", compositor_name(compositor), known.len());

        // Find libveil_capture.so for LD_PRELOAD injection
        let preload_so = find_sibling("libveil_capture.so");
        if let Some(ref p) = preload_so {
            eprintln!("[gui] LD_PRELOAD={}", p.display());
        }

        let mut cmd = Command::new(app);
        cmd.env("ACCESSIBILITY_ENABLED", "1")
           .env("GTK_MODULES", "gail:atk-bridge")
           .stdout(Stdio::null())
           .stderr(Stdio::null());

        if let Some(ref so) = preload_so {
            cmd.env("LD_PRELOAD", so);
        }

        let child     = cmd.spawn().unwrap_or_else(|e| panic!("failed to launch `{app}`: {e}"));
        let child_pid = child.id();
        eprintln!("[gui] launched pid={}", child_pid);

        let win = match wait_for_new_window(&known, child_pid, compositor, window_timeout) {
            Some(w) => {
                eprintln!("[gui] window app_id='{}' title='{}' size={}×{}", w.app_id, w.title, w.w, w.h);
                w
            }
            None => {
                eprintln!("[gui] no window found — fullscreen fallback");
                WindowInfo {
                    pid:    child_pid,
                    app_id: app.to_string(),
                    title:  String::new(),
                    x:      0,
                    y:      0,
                    w:      1920,
                    h:      1080,
                }
            }
        };

        let geo = Geo { x: 0, y: 0, w: win.w, h: win.h };

        let latest_rgba: Arc<Mutex<(u32, u32, Vec<u8>)>> =
            Arc::new(Mutex::new((0, 0, Vec::new())));
        let latest_text: Arc<Mutex<Vec<TextCell>>> = Arc::new(Mutex::new(Vec::new()));

        let shm_path = format!("/dev/shm/veil_{}", child_pid);

        // ── Capture thread ────────────────────────────────────────────────
        {
            let rgba_ref  = Arc::clone(&latest_rgba);
            let frame_dur = Duration::from_secs_f64(1.0 / capture_fps.max(1) as f64);

            let shm_path_clone  = shm_path.clone();
            let win_app_id_cap  = win.app_id.clone();
            let win_pid_cap     = win.pid;
            let win_x_init      = win.x;
            let win_y_init      = win.y;
            let win_w_init      = win.w;
            let win_h_init      = win.h;
            let compositor_cap  = compositor;

            thread::spawn(move || {
                // Give LD_PRELOAD a moment to create the SHM file
                thread::sleep(Duration::from_millis(200));

                // ── Priority 1: SHM (LD_PRELOAD) ──────────────────────────
                // Only enter if the SHM file exists AND opens successfully.
                // If either fails, fall through — never return early.
                if std::path::Path::new(&shm_path_clone).exists() {
                    match ShmCapture::open(child_pid) {
                        Some(mut shm) => {
                            eprintln!("[capture] SHM (LD_PRELOAD)");
                            loop {
                                let tick = Instant::now();
                                if let Some((w, h, _stride, pixels)) = shm.read_frame() {
                                    *rgba_ref.lock().unwrap() = (w, h, pixels);
                                }
                                if let Some(rem) = frame_dur.checked_sub(tick.elapsed()) {
                                    thread::sleep(rem);
                                }
                            }
                        }
                        None => eprintln!("[capture] SHM file exists but open failed — skipping"),
                    }
                }

                // ── Priority 2: wlr-screencopy (full output + crop) ─────
                // Capture the full output each frame and crop to the window
                // region. Re-polls compositor IPC every 500ms for resize.
                match WaylandCapture::connect() {
                    Some(mut wc) => {
                        let scale = wc.output_scale();

                        let mut wx = win_x_init;
                        let mut wy = win_y_init;
                        let mut ww = win_w_init;
                        let mut wh = win_h_init;
                        let mut win_poll = Instant::now();
                        let poll_interval = Duration::from_millis(500);

                        eprintln!("[capture] wlr-screencopy full+crop '{}' ({}x{} @ {},{})",
                            win_app_id_cap, ww, wh, wx, wy);

                        loop {
                            let tick = Instant::now();

                            // Re-poll window bounds for live resize/move support
                            if win_poll.elapsed() >= poll_interval {
                                if let Some(w) = list_windows(compositor_cap)
                                    .into_iter()
                                    .find(|w| w.app_id == win_app_id_cap || w.pid == win_pid_cap)
                                {
                                    if w.x != wx || w.y != wy || w.w != ww || w.h != wh {
                                        eprintln!("[capture] window resized {}x{} @ {},{}",
                                            w.w, w.h, w.x, w.y);
                                        wx = w.x; wy = w.y; ww = w.w; wh = w.h;
                                    }
                                }
                                win_poll = Instant::now();
                            }

                            if let Some((fw, fh, full)) = wc.capture_full() {
                                let (cw, ch, cropped) = crop_rgba(
                                    &full, fw, fh, wx, wy, ww, wh, scale,
                                );
                                if cw > 0 && ch > 0 {
                                    *rgba_ref.lock().unwrap() = (cw, ch, cropped);
                                }
                            }

                            if let Some(rem) = frame_dur.checked_sub(tick.elapsed()) {
                                thread::sleep(rem);
                            } else {
                                thread::sleep(Duration::from_millis(16));
                            }
                        }
                    }

                    None => {
                        eprintln!("[capture] zwlr_screencopy_manager_v1 not available — idling");
                        loop { thread::sleep(Duration::from_millis(500)); }
                    }
                }
            });
        }

        // ── AT-SPI thread ─────────────────────────────────────────────────
        {
            let text_ref = Arc::clone(&latest_text);
            let win_pid  = win.pid;
            let g        = geo.clone();

            let script = std::env::temp_dir().join("veil_atspi.py");
            let _ = std::fs::write(&script, ATSPI_SCRIPT);

            thread::spawn(move || loop {
                if let Some(cells) = query_atspi(&script, win_pid, &g, cols, rows) {
                    *text_ref.lock().unwrap() = cells;
                }
                thread::sleep(Duration::from_millis(500));
            });
        }

        thread::sleep(Duration::from_millis(300));
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
