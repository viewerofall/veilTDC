// Luma encoder — for each terminal cell, nearest-neighbour sample the
// centre source pixel, compute Rec.601 luma, write to output buffer.
//
// Output layout: 1 × u32 per cell, row-major.  Only the low 8 bits are used.

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

    let px_x = col * p.src_w / p.cols;
    let px_y = row * p.src_h / p.rows;

    let px = textureLoad(src, vec2<i32>(i32(px_x), i32(px_y)), 0);

    // Rec. 601 luma coefficients
    let luma = px.r * 0.299 + px.g * 0.587 + px.b * 0.114;

    out[row * p.cols + col] = u32(luma * 255.0);
}
