mod tui;
mod gui;
mod capture_shm;
mod wayland_capture;

pub use tui::TermCompositor;
pub use gui::GuiCompositor;
pub use capture_shm::ShmCapture;
