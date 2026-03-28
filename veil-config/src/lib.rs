use mlua::Lua;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Quality {
    AsciiLuma,
    AsciiEdge,
    Sixel,
    Kitty,
}

impl Quality {
    pub fn from_str(s: &str) -> Self {
        match s {
            "kitty"      => Self::Kitty,
            "sixel"      => Self::Sixel,
            "ascii_edge" => Self::AsciiEdge,
            _            => Self::AsciiLuma,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kitty     => "kitty",
            Self::Sixel     => "sixel",
            Self::AsciiEdge => "ascii_edge",
            Self::AsciiLuma => "ascii_luma",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VeilConfig {
    pub quality: Quality,
    pub fps: u32,
}

impl Default for VeilConfig {
    fn default() -> Self {
        Self {
            quality: Quality::AsciiLuma,
            fps: 30,
        }
    }
}

/// Load config.lua. Falls back to defaults on any error.
pub fn load(path: &Path) -> VeilConfig {
    if !path.exists() {
        return VeilConfig::default();
    }

    let src = match std::fs::read_to_string(path) {
        Ok(s)  => s,
        Err(_) => return VeilConfig::default(),
    };

    let lua = Lua::new();
    if lua.load(&src).exec().is_err() {
        eprintln!("[config] failed to parse {:?}, using defaults", path);
        return VeilConfig::default();
    }

    let g = lua.globals();
    let quality = g.get::<String>("quality").unwrap_or_else(|_| "ascii_luma".into());
    let fps     = g.get::<u32>("fps").unwrap_or(30);

    VeilConfig {
        quality: Quality::from_str(&quality),
        fps,
    }
}
