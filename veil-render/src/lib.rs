use image::{imageops, DynamicImage, RgbaImage};

const LUMA_MAP: &[char] = &[
    ' ', '.', '\'', '`', '^', '"', ',', ':', ';', 'I', 'l', '!', 'i',
    '>', '<', '~', '+', '_', '-', '?', ']', '[', '}', '{', '1', ')',
    '(', '|', '\\', '/', 't', 'f', 'j', 'r', 'x', 'n', 'u', 'v', 'c',
    'z', 'X', 'Y', 'U', 'J', 'C', 'L', 'Q', '0', 'O', 'Z', 'm', 'w',
    'q', 'p', 'd', 'b', 'k', 'h', 'a', 'o', '*', '#', 'M', 'W', '&',
    '8', '%', 'B', '@', '$',
];

pub fn luma_to_char(luma: u8) -> char {
    LUMA_MAP[(luma as usize * (LUMA_MAP.len() - 1)) / 255]
}

// ── TUI path ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Cell {
    pub ch: char,
    pub luma: u8,
}

impl Cell {
    pub fn rendered(&self) -> char {
        if self.ch.is_ascii_graphic() { self.ch } else { luma_to_char(self.luma) }
    }
}

pub struct TermFrame {
    pub cells: Vec<Cell>,
    pub width: u16,
    pub height: u16,
}

pub fn render_chars(frame: &TermFrame) -> Vec<char> {
    frame.cells.iter().map(|c| c.rendered()).collect()
}

// ── GUI path ──────────────────────────────────────────────────────────────────

pub fn compute_luma(rgba: &[u8], src_w: u32, src_h: u32, cols: u16, rows: u16) -> Vec<u8> {
    let full = cols as usize * rows as usize;
    let img = match RgbaImage::from_raw(src_w, src_h, rgba.to_vec()) {
        Some(i) => i,
        None    => return vec![0; full],
    };
    DynamicImage::ImageRgba8(img)
        .resize_exact(cols as u32, rows as u32, imageops::FilterType::Triangle)
        .to_luma8()
        .pixels()
        .map(|p| p[0])
        .collect()
}

pub fn apply_hysteresis(stable: &mut Vec<u8>, current: &[u8], threshold: u8) -> bool {
    let mut changed = false;
    for (s, &c) in stable.iter_mut().zip(current.iter()) {
        if (*s as i16 - c as i16).abs() >= threshold as i16 {
            *s = c;
            changed = true;
        }
    }
    changed
}

/// Map stabilised luma values to characters, with cell-level edge detection.
///
/// Each cell is compared against its left/right/above/below neighbours.
/// A sharp luma jump between neighbours means there's a UI boundary running
/// through this cell — render it as `|`, `-`, or `+` instead of a luma char.
/// This makes buttons, panels, and window chrome visually recognisable.
pub fn luma_to_chars(luma: &[u8], cols: u16, rows: u16) -> Vec<char> {
    let c = cols as usize;
    let r = rows as usize;

    let get = |row: i32, col: i32| -> u8 {
        if row < 0 || col < 0 || row >= r as i32 || col >= c as i32 {
            return 128; // neutral border value
        }
        luma[(row as usize * c) + col as usize]
    };

    // A cell is a UI edge when the contrast across it (neighbour-to-neighbour)
    // exceeds this threshold. Tuned for typical UI chrome contrast (>30) while
    // ignoring smooth gradients and hysteresis-stabilised noise.
    const EDGE_T: u8 = 38;

    let mut out = Vec::with_capacity(c * r);
    for row in 0..r as i32 {
        for col in 0..c as i32 {
            let l = get(row, col);

            // Horizontal contrast (left→right) → indicates a vertical edge `|`
            let horiz = (get(row, col - 1) as i16 - get(row, col + 1) as i16).abs() as u8;
            // Vertical contrast (above→below) → indicates a horizontal edge `-`
            let vert  = (get(row - 1, col) as i16 - get(row + 1, col) as i16).abs() as u8;

            let ch = if horiz > EDGE_T && vert > EDGE_T {
                '+'
            } else if horiz > EDGE_T {
                '|'
            } else if vert > EDGE_T {
                '-'
            } else {
                luma_to_char(l)
            };

            out.push(ch);
        }
    }
    out
}

// ── Text overlay ──────────────────────────────────────────────────────────────

/// A text element placed at a specific terminal cell position.
/// Produced by the AT-SPI query and stamped over the luma/edge render.
#[derive(Clone)]
pub struct TextCell {
    pub col:  u16,
    pub row:  u16,
    pub text: String,
}

/// Stamp AT-SPI text elements over an already-rendered char grid.
/// Text is truncated at the right edge of the terminal.
pub fn apply_text_overlay(chars: &mut [char], text: &[TextCell], cols: u16) {
    for tc in text {
        let base = tc.row as usize * cols as usize + tc.col as usize;
        let space = cols as usize - tc.col as usize;
        for (i, ch) in tc.text.chars().take(space).enumerate() {
            if let Some(slot) = chars.get_mut(base + i) {
                // Only stamp printable ASCII — skip control chars and wide unicode
                if ch.is_ascii_graphic() || ch == ' ' {
                    *slot = ch;
                }
            }
        }
    }
}
