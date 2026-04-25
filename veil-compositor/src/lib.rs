mod tui;
mod gui;
mod capture_shm;
mod wayland_screencopy;

pub use tui::TermCompositor;
pub use gui::GuiCompositor;
pub use capture_shm::ShmCapture;
pub use wayland_screencopy::ScreencopyCapture;
