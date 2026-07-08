//! Session detection: what can we log the user into?
//!
//! Hardcoded entries first (Veil, the user's shell), then anything with a
//! .desktop file in the standard wayland/x11 session dirs.

use std::path::Path;

#[derive(Clone, Debug)]
pub struct SessionEntry {
    pub name: String,
    /// Shell command line, run as `<user shell> -lc "exec <exec>"`.
    /// Empty exec means "just a login shell".
    pub exec: String,
}

pub fn detect() -> Vec<SessionEntry> {
    let mut sessions = vec![
        SessionEntry { name: "Veil".into(), exec: "veil-host start".into() },
        SessionEntry { name: "Shell".into(), exec: String::new() },
    ];

    for dir in ["/usr/share/wayland-sessions", "/usr/share/xsessions"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "desktop") {
                if let Some(s) = from_desktop_file(&path) {
                    // skip duplicates by name (same DE in both dirs)
                    if !sessions.iter().any(|e| e.name == s.name) {
                        sessions.push(s);
                    }
                }
            }
        }
    }

    sessions
}

fn from_desktop_file(path: &Path) -> Option<SessionEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let exec = ini_get(&content, "Desktop Entry", "Exec")?;
    let name = ini_get(&content, "Desktop Entry", "Name").unwrap_or_else(|| {
        path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
    });
    Some(SessionEntry { name, exec })
}

fn ini_get(content: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == format!("[{section}]");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}
