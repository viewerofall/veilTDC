use std::collections::HashSet;
use std::fs::File;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use veil_render::TextCell;
use crate::wayland_capture::WaylandCapture;

#[allow(dead_code)]
const ATSPI_SCRIPT: &str = include_str!("../../atspi_query.py");

/* ── Wayland socket discovery via /proc ──────────────────────────────────── */

/// Find the wayland-* socket that cage (cage_pid) is currently listening on.
/// Reads /proc/<pid>/fd to get socket inodes, matches against /proc/net/unix.
fn find_cage_socket(cage_pid: u32, runtime_dir: &str) -> Option<String> {
    let fd_dir = format!("/proc/{}/fd", cage_pid);
    let mut inodes: HashSet<u64> = HashSet::new();

    if let Ok(entries) = std::fs::read_dir(&fd_dir) {
        for entry in entries.flatten() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                let s = target.to_string_lossy();
                // Kernel renders socket fds as "socket:[inode]"
                if let Some(rest) = s.strip_prefix("socket:[") {
                    if let Some(inode_str) = rest.strip_suffix("]") {
                        if let Ok(inode) = inode_str.parse::<u64>() {
                            inodes.insert(inode);
                        }
                    }
                }
            }
        }
    }

    if inodes.is_empty() { return None; }

    let prefix = format!("{}/wayland-", runtime_dir);
    let data = std::fs::read_to_string("/proc/net/unix").ok()?;

    for line in data.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 { continue; }
        let inode: u64 = parts[6].parse().unwrap_or(0);
        let path = parts[7];
        if inodes.contains(&inode) && path.starts_with(&prefix) {
            return Some(path.to_string());
        }
    }

    None
}

/// Poll until cage has a wayland socket open, or timeout.
fn wait_for_cage_socket(cage_pid: u32, runtime_dir: &str, timeout: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(path) = find_cage_socket(cage_pid, runtime_dir) {
            return Some(path);
        }
        thread::sleep(Duration::from_millis(50));
    }
    None
}

/* ── AT-SPI query ───────────────────────────────────────────────────────── */

#[derive(Clone)]
#[allow(dead_code)]
struct Geo { x: i32, y: i32, w: u32, h: u32 }

#[allow(dead_code)]
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

/* ── RGBA crop ──────────────────────────────────────────────────────────── */

#[allow(dead_code)]
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

/* ── GuiCompositor ──────────────────────────────────────────────────────── */

pub struct GuiCompositor {
    child:       Child,
    latest_rgba: Arc<Mutex<(u32, u32, Vec<u8>)>>,
    latest_text: Arc<Mutex<Vec<TextCell>>>,
    pub cage_socket: Option<String>,
}

impl GuiCompositor {
    pub fn launch(
        app:          &str,
        _cols:        u16,
        _rows:        u16,
        cage_timeout: Duration,
        capture_fps:  u32,
    ) -> Self {
        let cage_log_path = "/tmp/veil-cage.log";
        let cage_log = File::create(cage_log_path)
            .unwrap_or_else(|_| File::create("/dev/null").unwrap());

        let child = Command::new("cage")
            .arg("--")
            .arg(app)
            .stdout(Stdio::null())
            .stderr(cage_log)
            .spawn()
            .unwrap_or_else(|e| panic!("failed to launch cage: {e}"));
        let cage_pid = child.id();
        eprintln!("[gui] cage pid={} (log → {})", cage_pid, cage_log_path);

        let latest_rgba: Arc<Mutex<(u32, u32, Vec<u8>)>> =
            Arc::new(Mutex::new((0, 0, Vec::new())));
        let latest_text: Arc<Mutex<Vec<TextCell>>> = Arc::new(Mutex::new(Vec::new()));

        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());

        // Discover cage's socket via /proc/<pid>/fd + /proc/net/unix.
        // This avoids the stale-socket race where cage reuses a name that was
        // in our "existing" set from a previous dead instance.
        let cage_socket_path: Option<String> = match wait_for_cage_socket(cage_pid, &runtime_dir, cage_timeout) {
            Some(path) => {
                eprintln!("[gui] cage socket: {}", path);
                thread::sleep(Duration::from_millis(200));
                Some(path)
            }
            None => {
                eprintln!("[gui] cage socket not found within {:?}", cage_timeout);
                eprintln!("[gui] cage log: {}", cage_log_path);
                if let Ok(log) = std::fs::read_to_string(cage_log_path) {
                    for line in log.lines().filter(|l| l.contains("[ERROR]") || l.contains("Failed")) {
                        eprintln!("[cage] {}", line);
                    }
                }
                None
            }
        };

        if let Some(socket_path) = cage_socket_path.clone() {
            let rgba_ref  = Arc::clone(&latest_rgba);
            let frame_dur = Duration::from_secs_f64(1.0 / capture_fps.max(1) as f64);

            thread::spawn(move || {
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

        Self { child, latest_rgba, latest_text, cage_socket: cage_socket_path }
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
