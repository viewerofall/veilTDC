mod tui;
mod gui;
mod capture_shm;
mod wayland_capture;
pub mod capture_zig; // Zig FFI — see build.rs setup instructions below

pub use tui::TermCompositor;
pub use gui::GuiCompositor;
pub use capture_shm::ShmCapture;
// pub use capture_zig::{xrgb_to_rgba, crop_rgba, ShmRing}; // TODO: enable once build.rs wired
