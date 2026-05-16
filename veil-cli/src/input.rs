use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::{sync::mpsc, thread, time::Duration};
use veil_compositor::{InputCmd, WaylandInput, evdev_keycode, keycodes, xkb_mod, btn};

#[derive(Debug, Clone)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
}

pub fn spawn_input_thread() -> mpsc::Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            if event::poll(Duration::from_millis(16)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(k))        => { let _ = tx.send(InputEvent::Key(k)); }
                    Ok(Event::Mouse(m))      => { let _ = tx.send(InputEvent::Mouse(m)); }
                    Ok(Event::Resize(w, h))  => { let _ = tx.send(InputEvent::Resize(w, h)); }
                    _                        => {}
                }
            }
        }
    });
    rx
}

/// XKB modifier bitmask from crossterm KeyModifiers.
fn xkb_mods(m: KeyModifiers) -> u32 {
    let mut bits: u32 = 0;
    if m.contains(KeyModifiers::SHIFT)   { bits |= xkb_mod::SHIFT; }
    if m.contains(KeyModifiers::CONTROL) { bits |= xkb_mod::CONTROL; }
    if m.contains(KeyModifiers::ALT)     { bits |= xkb_mod::ALT; }
    if m.contains(KeyModifiers::SUPER)   { bits |= xkb_mod::SUPER; }
    bits
}

/// Convert a crossterm KeyEvent to (keycode, modifier_bits). Returns None if unmappable.
fn key_to_input(ev: &KeyEvent) -> Option<(u32, u32)> {
    let (needs_shift, canonical) = match ev.code {
        KeyCode::Char(c) if c.is_ascii_uppercase() =>
            (true, KeyCode::Char(c.to_ascii_lowercase())),
        KeyCode::Char(c @ ('!'|'@'|'#'|'$'|'%'|'^'|'&'|'*'|'('|')'|
                            '_'|'+'|'{'|'}'|'|'|':'|'"'|'~'|'<'|'>'|'?')) => {
            let unshifted = match c {
                '!' => '1', '@' => '2', '#' => '3', '$' => '4', '%' => '5',
                '^' => '6', '&' => '7', '*' => '8', '(' => '9', ')' => '0',
                '_' => '-', '+' => '=', '{' => '[', '}' => ']', '|' => '\\',
                ':' => ';', '"' => '\'', '~' => '`', '<' => ',', '>' => '.',
                '?' => '/',  _ => c,
            };
            (true, KeyCode::Char(unshifted))
        }
        other => (false, other),
    };

    let keycode = match canonical {
        KeyCode::Char(c) => evdev_keycode(c)?,
        KeyCode::Backspace => keycodes::BACKSPACE,
        KeyCode::Tab       => keycodes::TAB,
        KeyCode::Enter     => keycodes::ENTER,
        KeyCode::Esc       => keycodes::ESC,
        KeyCode::Delete    => keycodes::DELETE,
        KeyCode::Insert    => keycodes::INSERT,
        KeyCode::Home      => keycodes::HOME,
        KeyCode::End       => keycodes::END,
        KeyCode::PageUp    => keycodes::PAGE_UP,
        KeyCode::PageDown  => keycodes::PAGE_DOWN,
        KeyCode::Left      => keycodes::LEFT,
        KeyCode::Right     => keycodes::RIGHT,
        KeyCode::Up        => keycodes::UP,
        KeyCode::Down      => keycodes::DOWN,
        KeyCode::F(n) if (1..=12).contains(&n) => keycodes::F[n as usize],
        _ => return None,
    };

    let mut mods = xkb_mods(ev.modifiers);
    if needs_shift { mods |= xkb_mod::SHIFT; }
    Some((keycode, mods))
}

/// Forward a terminal input event into cage via WaylandInput.
/// Returns Some((cols, rows)) if the terminal was resized.
pub fn forward_event(ev: &InputEvent, input: &WaylandInput, cols: u16, rows: u16) -> Option<(u16, u16)> {
    match ev {
        InputEvent::Key(k) => {
            if let Some((keycode, mods)) = key_to_input(k) {
                use crossterm::event::KeyEventKind;
                let pressed = k.kind != KeyEventKind::Release;
                input.send(InputCmd::Key { keycode, mods, pressed });
            }
            None
        }
        InputEvent::Mouse(m) => {
            // Always update pointer position first (matches texttop/browsh's "move before click" pattern).
            // In halfblock mode each terminal row covers 2 source pixels — use rows*2 as height extent
            // so the y coordinate maps to the correct cage pixel position.
            input.send(InputCmd::PointerMotionAbs {
                x:      m.column as u32,
                y:      m.row as u32 * 2,
                width:  cols as u32,
                height: rows as u32 * 2,
            });
            match m.kind {
                MouseEventKind::Down(b) => input.send(InputCmd::PointerButton {
                    button: mouse_btn(b), pressed: true,
                }),
                MouseEventKind::Up(b) => input.send(InputCmd::PointerButton {
                    button: mouse_btn(b), pressed: false,
                }),
                MouseEventKind::ScrollUp   => input.send(InputCmd::PointerAxisScroll { steps: -1 }),
                MouseEventKind::ScrollDown => input.send(InputCmd::PointerAxisScroll { steps:  1 }),
                MouseEventKind::Moved | MouseEventKind::Drag(_) => {} // motion already sent above
                _ => {}
            }
            None
        }
        InputEvent::Resize(w, h) => Some((*w, *h)),
    }
}

fn mouse_btn(b: MouseButton) -> u32 {
    match b {
        MouseButton::Left   => btn::LEFT,
        MouseButton::Right  => btn::RIGHT,
        MouseButton::Middle => btn::MIDDLE,
    }
}
