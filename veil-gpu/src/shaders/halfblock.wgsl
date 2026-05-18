// Halfblock encoder — for each terminal cell, nearest-neighbour sample the
// top and bottom source pixels, pack fg/bg RGB into the output buffer.
//
// Output layout: 2 × u32 per cell, row-major.
//   word0 = fg.r | (fg.g << 8) | (fg.b << 16)
//   word1 = bg.r | (bg.g << 8) | (bg.b << 16)

struct Params {
    src_w : u32,
    src_h : u32,
    cols  : u32,
    rows  : u32,
}

@group(0) @binding(0) var src : texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> out : array<u32>;
@group(0) @binding(2) var<uniform> p : Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let col = id.x;
    let row = id.y;
    if col >= p.cols || row >= p.rows { return; }

    // Each terminal row maps to two source pixel rows via ▀ (top=fg, bot=bg).
    let eff_w = p.cols;
    let eff_h = p.rows * 2u;

    let px_x  = col           * p.src_w / eff_w;
    let top_y = (row * 2u)     * p.src_h / eff_h;
    let bot_y = (row * 2u + 1u) * p.src_h / eff_h;

    let fg = textureLoad(src, vec2<i32>(i32(px_x), i32(top_y)), 0);
    let bg = textureLoad(src, vec2<i32>(i32(px_x), i32(bot_y)), 0);

    let fr = u32(fg.r * 255.0);
    let fg_ = u32(fg.g * 255.0);
    let fb = u32(fg.b * 255.0);
    let br = u32(bg.r * 255.0);
    let bg_ = u32(bg.g * 255.0);
    let bb = u32(bg.b * 255.0);

    let idx = (row * p.cols + col) * 2u;
    out[idx]      = fr | (fg_ << 8u) | (fb << 16u);
    out[idx + 1u] = br | (bg_ << 8u) | (bb << 16u);
}
