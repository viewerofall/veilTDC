use mlua::Lua;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Quality {
    Auto,
    Pixel,      // color halfblock (▀, 24-bit truecolor) — best for GUI apps
    AsciiLuma,  // greyscale luma chars — works everywhere
    AsciiEdge,  // luma + edge detection overlay
    Sixel,      // sixel inline images (not yet implemented)
    Kitty,      // kitty graphics protocol (not yet implemented)
}

impl Quality {
    pub fn from_str(s: &str) -> Self {
        match s {
            "auto"       => Self::Auto,
            "pixel"      => Self::Pixel,
            "ascii"      => Self::AsciiLuma,
            "ascii_luma" => Self::AsciiLuma,
            "ascii_edge" => Self::AsciiEdge,
            "sixel"      => Self::Sixel,
            "kitty"      => Self::Kitty,
            _            => Self::Auto,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto      => "auto",
            Self::Pixel     => "pixel",
            Self::AsciiLuma => "ascii_luma",
            Self::AsciiEdge => "ascii_edge",
            Self::Sixel     => "sixel",
            Self::Kitty     => "kitty",
        }
    }
}

/// Detect the best supported render mode from terminal environment variables.
pub fn detect_quality() -> Quality {
    let term      = std::env::var("TERM").unwrap_or_default();
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();

    // Kitty graphics protocol
    if term == "xterm-kitty" {
        return Quality::Kitty;
    }
    if let Ok(prog) = std::env::var("TERM_PROGRAM") {
        if prog == "WezTerm" || prog.to_lowercase().contains("wezterm") {
            return Quality::Kitty;
        }
    }

    // Sixel
    if term.contains("sixel") || term == "mlterm" {
        return Quality::Sixel;
    }

    // Truecolor — halfblock pixel rendering works great
    if colorterm == "truecolor" || colorterm == "24bit" {
        return Quality::Pixel;
    }

    Quality::AsciiLuma
}

#[derive(Debug, Clone)]
pub struct VeilConfig {
    pub quality:           Quality,
    pub fps:               u32,
    pub cage_timeout_secs: u32,
    pub input:             bool,
}

impl Default for VeilConfig {
    fn default() -> Self {
        Self {
            quality:           Quality::Auto,
            fps:               60,
            cage_timeout_secs: 8,
            input:             true,
        }
    }
}

impl VeilConfig {
    /// Resolve `Quality::Auto` to a concrete mode based on terminal capabilities.
    pub fn resolved_quality(&self) -> Quality {
        if self.quality == Quality::Auto {
            detect_quality()
        } else {
            self.quality.clone()
        }
    }
}

/// Load config from `path`. Falls back to defaults on any error or missing file.
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

    let d  = VeilConfig::default();
    let gl = lua.globals();

    VeilConfig {
        quality: gl.get::<String>("quality")
            .map(|s| Quality::from_str(&s))
            .unwrap_or(d.quality),
        fps: gl.get::<u32>("fps")
            .unwrap_or(d.fps),
        cage_timeout_secs: gl.get::<u32>("cage_timeout_secs")
            .unwrap_or(d.cage_timeout_secs),
        input: gl.get::<bool>("input")
            .unwrap_or(d.input),
    }
}
