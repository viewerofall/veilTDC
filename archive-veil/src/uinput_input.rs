/// uinput-based virtual keyboard and mouse for cage input injection.
///
/// Creates a /dev/uinput virtual device. wlroots monitors udev for new
/// input devices via libinput, so cage picks this up dynamically — no
/// Wayland virtual-keyboard/pointer protocol support required.
///
/// Requires the user to be in the `input` group, or a udev rule granting
/// access to /dev/uinput.
use std::{sync::mpsc, thread, time::Duration};

use evdev::{
    uinput::VirtualDevice,
    AbsInfo, AbsoluteAxisCode, AttributeSet, InputEvent,
    KeyCode, RelativeAxisCode, UinputAbsSetup,
};

use crate::wayland_input::InputCmd;

// Raw EV_* type codes
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;

pub struct UInputHandle {
    tx: mpsc::SyncSender<InputCmd>,
}

impl UInputHandle {
    pub fn new() -> Option<Self> {
        let dev = match build_device() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[uinput] failed to create device: {} (are you in the 'input' group?)", e);
                return None;
            }
        };
        eprintln!("[uinput] virtual device created — waiting for udev/libinput discovery");
        thread::sleep(Duration::from_millis(300));
        eprintln!("[uinput] ready");

        let (tx, rx) = mpsc::sync_channel::<InputCmd>(64);

        thread::spawn(move || {
            let mut dev = dev;
            let mut held_mods: u32 = 0;
            loop {
                match rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(cmd) => { let _ = dispatch(&mut dev, cmd, &mut held_mods); }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });

        Some(Self { tx })
    }

    pub fn send(&self, cmd: InputCmd) {
        let _ = self.tx.try_send(cmd);
    }
}

/* ── Key set ─────────────────────────────────────────────────────────────── */

fn all_keys() -> AttributeSet<KeyCode> {
    let mut s = AttributeSet::<KeyCode>::new();
    for k in [
        // Letters
        KeyCode::KEY_A, KeyCode::KEY_B, KeyCode::KEY_C, KeyCode::KEY_D, KeyCode::KEY_E,
        KeyCode::KEY_F, KeyCode::KEY_G, KeyCode::KEY_H, KeyCode::KEY_I, KeyCode::KEY_J,
        KeyCode::KEY_K, KeyCode::KEY_L, KeyCode::KEY_M, KeyCode::KEY_N, KeyCode::KEY_O,
        KeyCode::KEY_P, KeyCode::KEY_Q, KeyCode::KEY_R, KeyCode::KEY_S, KeyCode::KEY_T,
        KeyCode::KEY_U, KeyCode::KEY_V, KeyCode::KEY_W, KeyCode::KEY_X, KeyCode::KEY_Y,
        KeyCode::KEY_Z,
        // Numbers
        KeyCode::KEY_1, KeyCode::KEY_2, KeyCode::KEY_3, KeyCode::KEY_4, KeyCode::KEY_5,
        KeyCode::KEY_6, KeyCode::KEY_7, KeyCode::KEY_8, KeyCode::KEY_9, KeyCode::KEY_0,
        // Punctuation
        KeyCode::KEY_MINUS, KeyCode::KEY_EQUAL, KeyCode::KEY_LEFTBRACE, KeyCode::KEY_RIGHTBRACE,
        KeyCode::KEY_BACKSLASH, KeyCode::KEY_SEMICOLON, KeyCode::KEY_APOSTROPHE,
        KeyCode::KEY_GRAVE, KeyCode::KEY_COMMA, KeyCode::KEY_DOT, KeyCode::KEY_SLASH,
        KeyCode::KEY_SPACE,
        // Modifiers
        KeyCode::KEY_LEFTSHIFT,  KeyCode::KEY_RIGHTSHIFT,
        KeyCode::KEY_LEFTCTRL,   KeyCode::KEY_RIGHTCTRL,
        KeyCode::KEY_LEFTALT,    KeyCode::KEY_RIGHTALT,
        KeyCode::KEY_LEFTMETA,   KeyCode::KEY_RIGHTMETA,
        // Nav / editing
        KeyCode::KEY_ESC, KeyCode::KEY_TAB, KeyCode::KEY_BACKSPACE, KeyCode::KEY_ENTER,
        KeyCode::KEY_INSERT, KeyCode::KEY_DELETE, KeyCode::KEY_HOME, KeyCode::KEY_END,
        KeyCode::KEY_PAGEUP, KeyCode::KEY_PAGEDOWN,
        KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT, KeyCode::KEY_UP, KeyCode::KEY_DOWN,
        // F-keys
        KeyCode::KEY_F1,  KeyCode::KEY_F2,  KeyCode::KEY_F3,  KeyCode::KEY_F4,
        KeyCode::KEY_F5,  KeyCode::KEY_F6,  KeyCode::KEY_F7,  KeyCode::KEY_F8,
        KeyCode::KEY_F9,  KeyCode::KEY_F10, KeyCode::KEY_F11, KeyCode::KEY_F12,
        // Mouse buttons (EV_KEY range in evdev)
        KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE,
    ] { s.insert(k); }
    s
}

/* ── Device construction ─────────────────────────────────────────────────── */

fn build_device() -> std::io::Result<VirtualDevice> {
    let keys = all_keys();

    let mut rel = AttributeSet::<RelativeAxisCode>::new();
    rel.insert(RelativeAxisCode::REL_WHEEL);

    let abs_x = UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, AbsInfo::new(0, 0, 65535, 0, 0, 1));
    let abs_y = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, AbsInfo::new(0, 0, 65535, 0, 0, 1));

    VirtualDevice::builder()?
        .name("veil-input")
        .with_keys(&keys)?
        .with_absolute_axis(&abs_x)?
        .with_absolute_axis(&abs_y)?
        .with_relative_axes(&rel)?
        .build()
}

/* ── Modifier bit → key code mapping ─────────────────────────────────────── */

const MOD_MAP: &[(u32, u16)] = &[
    (1,  KeyCode::KEY_LEFTSHIFT.code()),
    (4,  KeyCode::KEY_LEFTCTRL.code()),
    (8,  KeyCode::KEY_LEFTALT.code()),
    (64, KeyCode::KEY_LEFTMETA.code()),
];

fn ev(type_: u16, code: u16, value: i32) -> InputEvent {
    InputEvent::new(type_, code, value)
}

fn syn() -> InputEvent { ev(EV_SYN, 0, 0) }

/* ── Event dispatch ──────────────────────────────────────────────────────── */

fn dispatch(dev: &mut VirtualDevice, cmd: InputCmd, held_mods: &mut u32) -> std::io::Result<()> {
    match cmd {
        InputCmd::Key { keycode, mods, pressed } => {
            let mut evs: Vec<InputEvent> = Vec::with_capacity(8);
            if pressed {
                for &(bit, mod_code) in MOD_MAP {
                    if mods & bit != 0 && *held_mods & bit == 0 {
                        evs.push(ev(EV_KEY, mod_code, 1));
                    }
                }
                *held_mods = mods;
                evs.push(ev(EV_KEY, keycode as u16, 1));
            } else {
                evs.push(ev(EV_KEY, keycode as u16, 0));
                for &(bit, mod_code) in MOD_MAP {
                    if *held_mods & bit != 0 && mods & bit == 0 {
                        evs.push(ev(EV_KEY, mod_code, 0));
                    }
                }
                *held_mods = mods;
            }
            evs.push(syn());
            dev.emit(&evs)?;
        }

        InputCmd::PointerButton { button, pressed } => {
            dev.emit(&[
                ev(EV_KEY, button as u16, if pressed { 1 } else { 0 }),
                syn(),
            ])?;
        }

        InputCmd::PointerMotionAbs { x, y, width, height } => {
            let ax = (x as u64 * 65535 / width.max(1) as u64) as i32;
            let ay = (y as u64 * 65535 / height.max(1) as u64) as i32;
            dev.emit(&[
                ev(EV_ABS, AbsoluteAxisCode::ABS_X.0, ax),
                ev(EV_ABS, AbsoluteAxisCode::ABS_Y.0, ay),
                syn(),
            ])?;
        }

        InputCmd::PointerAxisScroll { steps } => {
            dev.emit(&[
                ev(EV_REL, RelativeAxisCode::REL_WHEEL.0, -steps),
                syn(),
            ])?;
        }
    }
    Ok(())
}
