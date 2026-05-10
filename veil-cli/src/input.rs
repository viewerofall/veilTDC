use crossterm::event::{self, Event, KeyEvent, KeyCode, KeyModifiers, MouseEvent, MouseEventKind, MouseButton};
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Raw input event from terminal/keyboard.
#[derive(Debug, Clone)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

/// Spawn an input thread that reads terminal events and sends them over a channel.
/// Returns an mpsc receiver for input events.
pub fn spawn_input_thread() -> mpsc::Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let _ = enable_mouse_support();
        loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    let _ = tx.send(InputEvent::Key(key));
                } else if let Ok(Event::Mouse(mouse)) = event::read() {
                    let _ = tx.send(InputEvent::Mouse(mouse));
                }
            }
        }
    });

    rx
}

/// Enable mouse support (basic crossterm setup).
fn enable_mouse_support() -> io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    Ok(())
}

/// Serialize input event to a simple text format for passing to apps or portal.
/// Format: "K:char:mods\n" or "M:button:x:y\n"
pub fn serialize_input(ev: &InputEvent) -> String {
    match ev {
        InputEvent::Key(key) => {
            format!("K:{:?}:{:?}\n", key.code, key.modifiers)
        }
        InputEvent::Mouse(mouse) => {
            let button = match mouse.kind {
                MouseEventKind::Down(b) => format!("{:?}", b),
                MouseEventKind::Up(b) => format!("{:?}", b),
                _ => "move".to_string(),
            };
            format!("M:{}:{}:{}\n", button, mouse.column, mouse.row)
        }
    }
}

// ── Portal D-Bus integration ───────────────────────────────────────────────

/// Inject keyboard keysym via xdg-desktop-portal RemoteDesktop.
/// Tries multiple backends and falls back to xdotool.
pub fn inject_key_via_portal_blocking(keysym: u32, pressed: bool) -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    // Try xdg-desktop-portal RemoteDesktop (GNOME/KDE path)
    let sym_str = format!("{}", keysym);
    let press_str = if pressed { "1" } else { "0" };

    if let Ok(status) = Command::new("dbus-send")
        .args(&[
            "--session",
            "--print-reply",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.RemoteDesktop.InjectKeyboardKeysym",
            &format!("uint32:{}", sym_str),
            &format!("uint32:{}", press_str),
        ])
        .output()
    {
        if status.status.success() {
            return Ok(());
        }
    }

    // Fallback: try xdotool (XWayland)
    let keysym_name = keysym_to_name(keysym);
    if !keysym_name.is_empty() {
        let action = if pressed { "keydown" } else { "keyup" };
        if let Ok(status) = Command::new("xdotool")
            .args(&[action, keysym_name])
            .output()
        {
            if status.status.success() {
                return Ok(());
            }
        }
    }

    // Fallback: use ydotool (Wayland-native)
    if let Ok(status) = Command::new("ydotool")
        .args(&["key", &if pressed {
            format!("{}:1", keysym)
        } else {
            format!("{}:0", keysym)
        }])
        .output()
    {
        if status.status.success() {
            return Ok(());
        }
    }

    Err("All input injection methods failed".into())
}

/// Inject pointer button via xdg-desktop-portal RemoteDesktop.
pub fn inject_pointer_button_blocking(button: i32, pressed: bool) -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    // Try portal first
    let btn_str = format!("{}", button);
    let press_str = if pressed { "1" } else { "0" };

    if let Ok(status) = Command::new("dbus-send")
        .args(&[
            "--session",
            "--print-reply",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.RemoteDesktop.InjectPointerButton",
            &format!("int32:{}", btn_str),
            &format!("uint32:{}", press_str),
        ])
        .output()
    {
        if status.status.success() {
            return Ok(());
        }
    }

    // Fallback: xdotool
    let btn_name = match button {
        1 => "1",
        3 => "3",
        2 => "2",
        _ => return Err(format!("Unknown button: {}", button).into()),
    };

    if let Ok(status) = Command::new("xdotool")
        .args(&[if pressed { "mousedown" } else { "mouseup" }, btn_name])
        .output()
    {
        if status.status.success() {
            return Ok(());
        }
    }

    Err("All pointer injection methods failed".into())
}

/// Convert X11 keysym to xdotool key name.
fn keysym_to_name(keysym: u32) -> &'static str {
    match keysym {
        32 => "space",
        0xff08 => "BackSpace",
        0xff09 => "Tab",
        0xff0d => "Return",
        0xff1b => "Escape",
        0xff50 => "Home",
        0xff57 => "End",
        0xff55 => "Prior",
        0xff56 => "Next",
        0xff51 => "Left",
        0xff53 => "Right",
        0xff52 => "Up",
        0xff54 => "Down",
        0xffff => "Delete",
        0xffbe..=0xffc9 => "F1", // Simplified - would need full mapping
        _ => "",
    }
}

/// Convert crossterm KeyCode to X11 keysym.
fn keycode_to_keysym(code: KeyCode) -> Option<u32> {
    match code {
        KeyCode::Char(c) => {
            let keysym = match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' => c as u32,
                ' ' => 0xff80,  // space
                '\n' => 0xff0d, // Return
                '\t' => 0xff09, // Tab
                _ => return None,
            };
            Some(keysym)
        }
        KeyCode::Backspace => Some(0xff08),
        KeyCode::Enter => Some(0xff0d),
        KeyCode::Tab => Some(0xff09),
        KeyCode::Esc => Some(0xff1b),
        KeyCode::Home => Some(0xff50),
        KeyCode::End => Some(0xff57),
        KeyCode::PageUp => Some(0xff55),
        KeyCode::PageDown => Some(0xff56),
        KeyCode::Left => Some(0xff51),
        KeyCode::Right => Some(0xff53),
        KeyCode::Up => Some(0xff52),
        KeyCode::Down => Some(0xff54),
        KeyCode::Delete => Some(0xffff),
        KeyCode::F(n) if n <= 12 => Some(0xffbe + (n as u32 - 1)),
        _ => None,
    }
}

/// Convert crossterm MouseButton to xdg-desktop-portal button code (1=left, 2=middle, 3=right).
fn mousebutton_to_portal(button: MouseButton) -> i32 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 3,
        MouseButton::Middle => 2,
    }
}

/// Handle a single input event: inject via portal (spawned in background thread).
pub fn handle_input_event(ev: &InputEvent) -> Option<()> {
    match ev {
        InputEvent::Key(key) => {
            if let Some(keysym) = keycode_to_keysym(key.code) {
                let pressed = !matches!(key.kind, crossterm::event::KeyEventKind::Release);
                thread::spawn(move || {
                    let _ = inject_key_via_portal_blocking(keysym, pressed);
                });
            }
        }
        InputEvent::Mouse(mouse) => {
            if let MouseEventKind::Down(btn) | MouseEventKind::Up(btn) = mouse.kind {
                let portal_btn = mousebutton_to_portal(btn);
                let pressed = matches!(mouse.kind, MouseEventKind::Down(_));
                thread::spawn(move || {
                    let _ = inject_pointer_button_blocking(portal_btn, pressed);
                });
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_key() {
        let key = KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        );
        let ev = InputEvent::Key(key);
        let s = serialize_input(&ev);
        assert!(s.contains("K:") && s.contains("NONE"));
    }

    #[test]
    fn test_serialize_mouse() {
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 10,
            row: 20,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        let ev = InputEvent::Mouse(mouse);
        let s = serialize_input(&ev);
        assert!(s.contains("M:") && s.contains("10") && s.contains("20"));
    }
}
