mod tui;
mod gui;
mod wayland_capture;
pub mod wayland_input;
mod uinput_input;

pub use tui::TermCompositor;
pub use gui::GuiCompositor;
pub use wayland_input::{WaylandInput, InputCmd, evdev_keycode, keycodes, xkb_mod, btn};
pub use uinput_input::UInputHandle;
