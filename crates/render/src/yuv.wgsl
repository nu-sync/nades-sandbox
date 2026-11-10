// A presented frame as NV12 (yuv.rs): rows 0..height are luma, the rows after them chroma with U and V interleaved.
// Integers throughout: swscale's own coefficients, shifts and vertical chroma filter, so the bytes are FFmpeg's.

struct Params {
    size: vec2<u32>,
    paints: u32,
    pad: u32,
    rects: array<vec4<i32>, 8>,
    colors: array<vec4<i32>, 8>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var frame: texture_2d<f32>;

@vertex
fn fullscreen(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

// The frame's stored bytes at a pixel (rows clamped to the frame), under the overlay's rectangles.
fn rgb(x: i32, y: i32) -> vec3<i32> {
    let p = vec2<i32>(x, clamp(y, 0, i32(params.size.y) - 1));
    var value = vec3<i32>(textureLoad(frame, p, 0).rgb * 255.0 + 0.5);
    for (var k = 0u; k < params.paints; k++) {
        let r = params.rects[k];
        if p.x >= r.x && p.y >= r.y && p.x < r.z && p.y < r.w {
            value = params.colors[k].rgb;
        }
    }
    return value;
}

// A row's chroma at 15 bits from a pair of pixels across: U, V.
fn chroma(x: i32, y: i32) -> vec2<i32> {
    let s = rgb(x, y) + rgb(x + 1, y);
    return vec2<i32>(
        ((-4865 * s.r - 9528 * s.g + 14392 * s.b + (0x4001 << 9u)) >> 10u) << 1u,
        ((14392 * s.r - 12061 * s.g - 2332 * s.b + (0x4001 << 9u)) >> 10u) << 1u,
    );
}

@fragment
fn nv12(@builtin(position) at: vec4<f32>) -> @location(0) u32 {
    let x = i32(at.x);
    let y = i32(at.y);
    let height = i32(params.size.y);
    if y < height {
        let c = rgb(x, y);
        return u32(min(255, (((8414 * c.r + 16519 * c.g + 3208 * c.b + 0x80100) >> 9u) + 32) >> 6u));
    }
    // swscale's bicubic at half size down the rows: eight taps, 4096 in all
    var taps = array<i32, 8>(-58, -172, 492, 1786, 1786, 492, -172, -58);
    let row = 2 * (y - height) - 3;
    let left = x & ~1;
    var sum = vec2<i32>(0);
    for (var k = 0; k < 8; k++) {
        sum += taps[k] * chroma(left, row + k);
    }
    let uv = clamp((sum + (64 << 12u)) >> vec2<u32>(19u), vec2<i32>(0), vec2<i32>(255));
    return u32(select(uv.x, uv.y, (x & 1) == 1));
}
