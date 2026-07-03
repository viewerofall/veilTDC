//! Dwindle tiling layout — Combo 3.
//!
//! veil-host tiles up to four toplevels. The window *order* is owned by
//! `State.toplevels`; this module owns only what can't be derived from that
//! list — which window has focus, and whether the primary split is flipped —
//! plus the pure geometry that turns `(count, width, height)` into rects.
//!
//! The dwindle progression (capped at 4, per the v2.0 spec):
//!   1 → fullscreen
//!   2 → left / right split          (flip: top / bottom)
//!   3 → left half + right column halved   (flip: top half + bottom row halved)
//!   4 → 2×2 grid
//! A 5th+ window stacks on the last cell (we cap at 4 visually).

/// A window rectangle in compositor pixel space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn center(&self) -> (i32, i32) {
        (self.x + self.w as i32 / 2, self.y + self.h as i32 / 2)
    }
}

/// Direction for spatial focus movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// `split_ratio` bounds — never let the primary pane shrink to nothing or
/// swallow the whole output.
const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;
/// Per-keypress adjustment for `resize_grow`/`resize_shrink`.
const RESIZE_STEP: f32 = 0.05;

/// Tiling state not derivable from the window list.
pub struct Layout {
    /// Index (into the live-toplevel order) of the focused window.
    pub focused: usize,
    /// `rotate_split` toggles this — flips the primary split axis.
    pub flip: bool,
    /// Primary pane's share of the OUTERMOST split only — adjusted by
    /// `resize_grow`/`resize_shrink` (Super+=/-). The secondary split (the
    /// stack's internal top/bottom for n=3, the whole n=4 grid) always stays
    /// a fixed 50/50; this is dwm-style "mfact", not per-pane resizing.
    pub split_ratio: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { focused: 0, flip: false, split_ratio: 0.5 }
    }
}

impl Layout {
    /// Compute a rect for each of `n` windows filling `w`×`h`. Always returns
    /// exactly `n` rects; windows past the 4th reuse the last cell.
    pub fn rects(&self, n: usize, w: u32, h: u32) -> Vec<Rect> {
        if n == 0 {
            return Vec::new();
        }
        let full = Rect { x: 0, y: 0, w, h };

        // Primary split (the outermost one) honors split_ratio; the
        // secondary split — subdividing whichever pane ISN'T primary —
        // always stays fixed 50/50. Both ratio and half variants computed
        // up front; each arm below picks whichever pair is "primary" for
        // its axis (columns when not flipped, rows when flipped).
        let ratio = self.split_ratio.clamp(MIN_RATIO, MAX_RATIO);
        let pw = ((w as f32) * ratio).round() as u32;
        let sw = w - pw;
        let ph = ((h as f32) * ratio).round() as u32;
        let sh = h - ph;
        let hw = w / 2;
        let hw2 = w - hw;
        let hh = h / 2;
        let hh2 = h - hh;

        let mut base: Vec<Rect> = match n {
            1 => vec![full],
            2 if self.flip => vec![
                Rect { x: 0, y: 0,          w, h: ph },
                Rect { x: 0, y: ph as i32,  w, h: sh },
            ],
            2 => vec![
                Rect { x: 0,         y: 0, w: pw, h },
                Rect { x: pw as i32, y: 0, w: sw, h },
            ],
            3 if self.flip => vec![
                Rect { x: 0,          y: 0,         w,       h: ph },
                Rect { x: 0,          y: ph as i32, w: hw,   h: sh },
                Rect { x: hw as i32,  y: ph as i32, w: hw2,  h: sh },
            ],
            3 => vec![
                Rect { x: 0,          y: 0,          w: pw, h },
                Rect { x: pw as i32,  y: 0,          w: sw, h: hh },
                Rect { x: pw as i32,  y: hh as i32,  w: sw, h: hh2 },
            ],
            // 4+ → 2×2 grid; extras stack on the bottom-right cell below.
            // Grid is NOT split_ratio-adjustable — always fixed 50/50, same
            // as before resize existed.
            _ => vec![
                Rect { x: 0,         y: 0,         w: hw,  h: hh },
                Rect { x: hw as i32, y: 0,         w: hw2, h: hh },
                Rect { x: 0,         y: hh as i32, w: hw,  h: hh2 },
                Rect { x: hw as i32, y: hh as i32, w: hw2, h: hh2 },
            ],
        };

        // Pad so every window gets a rect (extras reuse the last cell).
        let last = *base.last().unwrap();
        while base.len() < n {
            base.push(last);
        }
        base
    }

    /// Grow the primary pane's share of the outermost split.
    pub fn resize_grow(&mut self) {
        self.split_ratio = (self.split_ratio + RESIZE_STEP).min(MAX_RATIO);
    }

    /// Shrink the primary pane's share of the outermost split.
    pub fn resize_shrink(&mut self) {
        self.split_ratio = (self.split_ratio - RESIZE_STEP).max(MIN_RATIO);
    }

    /// Move focus to the nearest window in `dir`, using rect centers. No-op if
    /// there's no window that way.
    pub fn focus(&mut self, rects: &[Rect], dir: Dir) {
        if rects.is_empty() {
            return;
        }
        let cur = rects[self.focused.min(rects.len() - 1)];
        let (cx, cy) = cur.center();
        let mut best: Option<(usize, i64)> = None;
        for (i, r) in rects.iter().enumerate() {
            if i == self.focused {
                continue;
            }
            let (rx, ry) = r.center();
            let (dx, dy) = (rx - cx, ry - cy);
            // Require the candidate to lie predominantly in the asked direction.
            let ok = match dir {
                Dir::Left => dx < 0 && dx.abs() >= dy.abs(),
                Dir::Right => dx > 0 && dx.abs() >= dy.abs(),
                Dir::Up => dy < 0 && dy.abs() >= dx.abs(),
                Dir::Down => dy > 0 && dy.abs() >= dx.abs(),
            };
            if !ok {
                continue;
            }
            let dist = (dx as i64) * (dx as i64) + (dy as i64) * (dy as i64);
            if best.is_none_or(|(_, bd)| dist < bd) {
                best = Some((i, dist));
            }
        }
        if let Some((i, _)) = best {
            self.focused = i;
        }
    }

    /// Indices to swap so the focused window trades places with the next one
    /// (wrapping). Focus follows the moved window. Returns `None` for <2 windows.
    pub fn swap_next(&mut self, n: usize) -> Option<(usize, usize)> {
        if n < 2 {
            return None;
        }
        let a = self.focused.min(n - 1);
        let b = (a + 1) % n;
        self.focused = b;
        Some((a, b))
    }

    /// Flip the primary split axis (left/right ↔ top/bottom).
    pub fn rotate_split(&mut self) {
        self.flip = !self.flip;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_window_is_fullscreen() {
        let l = Layout::default();
        assert_eq!(l.rects(1, 100, 80), vec![Rect { x: 0, y: 0, w: 100, h: 80 }]);
    }

    #[test]
    fn two_windows_split_left_right() {
        let l = Layout::default();
        let r = l.rects(2, 100, 80);
        assert_eq!(r[0], Rect { x: 0, y: 0, w: 50, h: 80 });
        assert_eq!(r[1], Rect { x: 50, y: 0, w: 50, h: 80 });
    }

    #[test]
    fn flip_splits_top_bottom() {
        let l = Layout { focused: 0, flip: true, ..Default::default() };
        let r = l.rects(2, 100, 80);
        assert_eq!(r[0], Rect { x: 0, y: 0, w: 100, h: 40 });
        assert_eq!(r[1], Rect { x: 0, y: 40, w: 100, h: 40 });
    }

    #[test]
    fn three_windows_corner() {
        let l = Layout::default();
        let r = l.rects(3, 100, 80);
        assert_eq!(r[0], Rect { x: 0, y: 0, w: 50, h: 80 });
        assert_eq!(r[1], Rect { x: 50, y: 0, w: 50, h: 40 });
        assert_eq!(r[2], Rect { x: 50, y: 40, w: 50, h: 40 });
    }

    #[test]
    fn four_windows_grid() {
        let l = Layout::default();
        let r = l.rects(4, 100, 80);
        assert_eq!(r[0], Rect { x: 0, y: 0, w: 50, h: 40 });
        assert_eq!(r[1], Rect { x: 50, y: 0, w: 50, h: 40 });
        assert_eq!(r[2], Rect { x: 0, y: 40, w: 50, h: 40 });
        assert_eq!(r[3], Rect { x: 50, y: 40, w: 50, h: 40 });
    }

    #[test]
    fn odd_dimensions_tile_without_gaps() {
        // 101×81: halves must cover the full extent (no lost row/column).
        let l = Layout::default();
        let r = l.rects(2, 101, 81);
        assert_eq!(r[0].w + r[1].w, 101);
        assert_eq!(r[1].x, r[0].w as i32);
    }

    #[test]
    fn extras_reuse_last_cell() {
        let l = Layout::default();
        let r = l.rects(6, 100, 80);
        assert_eq!(r.len(), 6);
        assert_eq!(r[5], r[3]); // stacks on bottom-right
    }

    #[test]
    fn focus_moves_right_then_left() {
        let mut l = Layout::default();
        let r = l.rects(2, 100, 80); // [left, right]
        l.focus(&r, Dir::Right);
        assert_eq!(l.focused, 1);
        l.focus(&r, Dir::Left);
        assert_eq!(l.focused, 0);
    }

    #[test]
    fn focus_noop_when_nothing_that_way() {
        let mut l = Layout::default();
        let r = l.rects(2, 100, 80);
        l.focus(&r, Dir::Left); // already leftmost
        assert_eq!(l.focused, 0);
    }

    #[test]
    fn swap_next_wraps_and_follows_focus() {
        let mut l = Layout::default();
        assert_eq!(l.swap_next(3), Some((0, 1)));
        assert_eq!(l.focused, 1);
        l.focused = 2;
        assert_eq!(l.swap_next(3), Some((2, 0)));
        assert_eq!(l.focused, 0);
        assert_eq!(l.swap_next(1), None);
    }

    #[test]
    fn resize_grows_and_shrinks_primary_pane() {
        let mut l = Layout::default();
        l.resize_grow();
        let r = l.rects(2, 100, 80);
        assert!(r[0].w > 50); // primary pane bigger than default 50/50
        assert_eq!(r[0].w + r[1].w, 100); // still covers the full width

        let mut l = Layout::default();
        l.resize_shrink();
        let r = l.rects(2, 100, 80);
        assert!(r[0].w < 50);
        assert_eq!(r[0].w + r[1].w, 100);
    }

    #[test]
    fn resize_clamps_at_bounds() {
        let mut l = Layout::default();
        for _ in 0..50 { l.resize_grow(); }
        assert!((l.split_ratio - MAX_RATIO).abs() < f32::EPSILON);

        let mut l = Layout::default();
        for _ in 0..50 { l.resize_shrink(); }
        assert!((l.split_ratio - MIN_RATIO).abs() < f32::EPSILON);
    }

    #[test]
    fn resize_does_not_affect_2x2_grid() {
        let mut l = Layout::default();
        l.resize_grow();
        let r = l.rects(4, 100, 80);
        assert_eq!(r[0], Rect { x: 0, y: 0, w: 50, h: 40 });
        assert_eq!(r[1], Rect { x: 50, y: 0, w: 50, h: 40 });
    }

    #[test]
    fn resize_affects_primary_split_only_in_three_window_layout() {
        let mut l = Layout::default();
        l.resize_grow();
        let r = l.rects(3, 100, 80);
        assert!(r[0].w > 50); // primary column grew
        // secondary (stacked) column's internal top/bottom split stays 50/50
        assert_eq!(r[1].h, 40);
        assert_eq!(r[2].h, 40);
    }
}
