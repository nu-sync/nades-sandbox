// After the scene: the exposure the game adapts to, its bloom, and the present pass with the map's tone curve and colour
// lookup table (crates/render/src/post.rs). Ported from show-me-the-nades' web view (render/composite.ts, bloom.ts),
// itself Source 2 Viewer's post_processing.frag and BloomRenderer. Every pass is a full-screen triangle; one layout
// serves them all, the parameters at a dynamic offset.

struct Params {
    // the input's texel size, and a blur's step
    texel: vec4<f32>,
    // threshold: (−threshold / width, 1 / width, strength, the map's exposure); combine: (higher weight, lower weight)
    a: vec4<f32>,
    // exposure: (the scene value shown as middle grey, or 0; the minimum; the maximum); present: the map's exposure
    // scale (bias, compensation, a fixed exposure)
    b: vec4<f32>,
    // present: bloom strengths add, screen and blur normalized together, and blur's own
    bloom: vec4<f32>,
    // present: shoulder, linear strength, linear angle, toe strength
    curve: vec4<f32>,
    // present: toe numerator, toe denominator, white point, 1 / curve(white point)
    curve2: vec4<f32>,
    // present: lookup table weight, scale, offset; w 1 with bloom
    lut: vec4<f32>,
    pad: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var input: texture_2d<f32>;
@group(0) @binding(2) var lower: texture_2d<f32>;
@group(0) @binding(3) var exposure: texture_2d<f32>;
@group(0) @binding(4) var linear_clamp: sampler;
@group(0) @binding(5) var lut: texture_3d<f32>;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn fullscreen(@builtin(vertex_index) v: u32) -> Out {
    let xy = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u)) * 2.0 - 1.0;
    var out: Out;
    out.clip = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>(xy.x * 0.5 + 0.5, 0.5 - xy.y * 0.5);
    return out;
}

fn adapted() -> f32 {
    return textureLoad(exposure, vec2<i32>(0, 0), 0).r;
}

// --- exposure ---

fn log_luma(uv: vec2<f32>) -> f32 {
    let l = dot(textureSampleLevel(input, linear_clamp, uv, 0.0).rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    return clamp(log2(max(l, 0.005)), -8.0, 5.0);
}

// the frame's log2 luminance at 64×36, as the game's exposure histogram reads it
@fragment
fn luminance(in: Out) -> @location(0) vec4<f32> {
    let d = 0.25 / vec2<f32>(64.0, 36.0);
    let l = 0.25 * (log_luma(in.uv + vec2<f32>(-d.x, -d.y)) + log_luma(in.uv + vec2<f32>(d.x, -d.y)) + log_luma(in.uv + vec2<f32>(-d.x, d.y)) + log_luma(in.uv + vec2<f32>(d.x, d.y)));
    return vec4<f32>(l, 0.0, 0.0, 1.0);
}

// the exposure the frame adapts to, one texel: the scene value the map's curve shows as middle grey over the log-average
// luminance, clamped to the bounds (minimum then maximum, as the game does); a map that does not adapt gets its fixed one
@fragment
fn reduce(in: Out) -> @location(0) vec4<f32> {
    var total = 0.0;
    if params.b.x > 0.0 {
        for (var y = 0; y < 36; y++) {
            var row = 0.0;
            for (var x = 0; x < 64; x++) {
                row += textureLoad(input, vec2<i32>(x, y), 0).r;
            }
            total += row;
        }
    }
    let average = exp2(total / 2304.0);
    let target_exposure = select(1.0, params.b.x / max(average, 0.0001), params.b.x > 0.0);
    return vec4<f32>(max(min(target_exposure, params.b.z), params.b.y), 0.0, 0.0, 1.0);
}

// --- bloom ---

fn bloom_luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.3, 0.59, 0.11));
}

fn karis(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + bloom_luminance(c));
}

fn karis_average(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let wa = karis(a);
    let wb = karis(b);
    let wc = karis(c);
    let wd = karis(d);
    return (a * wa + b * wb + c * wc + d * wd) / (wa + wb + wc + wd);
}

fn tap(uv: vec2<f32>, offset: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(input, linear_clamp, uv + offset * params.texel.xy, 0.0).rgb;
}

// the quarter-size bright pass: Jimenez's 13-tap downsample with Karis weights, then the soft threshold
@fragment
fn threshold(in: Out) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let center = tap(uv, vec2<f32>(0.0));
    let tl = tap(uv, vec2<f32>(-1.0, -1.0));
    let tr = tap(uv, vec2<f32>(1.0, -1.0));
    let bl = tap(uv, vec2<f32>(-1.0, 1.0));
    let br = tap(uv, vec2<f32>(1.0, 1.0));
    let t = tap(uv, vec2<f32>(0.0, -2.0));
    let l = tap(uv, vec2<f32>(-2.0, 0.0));
    let r = tap(uv, vec2<f32>(2.0, 0.0));
    let b = tap(uv, vec2<f32>(0.0, 2.0));
    let tlf = tap(uv, vec2<f32>(-2.0, -2.0));
    let trf = tap(uv, vec2<f32>(2.0, -2.0));
    let blf = tap(uv, vec2<f32>(-2.0, 2.0));
    let brf = tap(uv, vec2<f32>(2.0, 2.0));
    let color = karis_average(tl, tr, bl, br) * 0.5 + (karis_average(tlf, t, l, center) + karis_average(trf, t, r, center) + karis_average(blf, b, l, center) + karis_average(brf, b, r, center)) * 0.125;
    let exposed = params.a.w * adapted();
    let weight = clamp(exposed * bloom_luminance(color) * params.a.y + params.a.x, 0.0, 1.0);
    return vec4<f32>(color * weight * params.a.z, 1.0);
}

@fragment
fn blur(in: Out) -> @location(0) vec4<f32> {
    let step = params.texel.zw;
    var sum = textureSampleLevel(input, linear_clamp, in.uv + step * -2.196197, 0.0).rgb * 0.121596;
    sum += textureSampleLevel(input, linear_clamp, in.uv + step * -0.430063, 0.0).rgb * 0.529213;
    sum += textureSampleLevel(input, linear_clamp, in.uv + step * 1.0, 0.0).rgb * 0.227595;
    sum += textureSampleLevel(input, linear_clamp, in.uv + step * 2.196197, 0.0).rgb * 0.121596;
    return vec4<f32>(sum, 1.0);
}

@fragment
fn copy(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleLevel(input, linear_clamp, in.uv, 0.0).rgb, 1.0);
}

// a level weighted, plus the level below it tent-filtered up
@fragment
fn combine(in: Out) -> @location(0) vec4<f32> {
    let t = params.texel.xy;
    let uv = in.uv;
    var below = textureSampleLevel(lower, linear_clamp, uv, 0.0).rgb * 0.25;
    below += (textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(-1.0, -1.0), 0.0).rgb + textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(1.0, -1.0), 0.0).rgb
        + textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(1.0, 1.0), 0.0).rgb + textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(-1.0, 1.0), 0.0).rgb) * 0.0625;
    below += (textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(0.0, -1.0), 0.0).rgb + textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(0.0, 1.0), 0.0).rgb
        + textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(-1.0, 0.0), 0.0).rgb + textureSampleLevel(lower, linear_clamp, uv + t * vec2<f32>(1.0, 0.0), 0.0).rgb) * 0.125;
    return vec4<f32>(textureSampleLevel(input, linear_clamp, uv, 0.0).rgb * params.a.x + below * params.a.y, 1.0);
}

// --- present ---

fn curve(x: vec3<f32>) -> vec3<f32> {
    let c = params.curve;
    let n = x * (c.x * x + c.y * c.z) + params.curve2.x * c.w;
    let d = x * (c.x * x + c.y) + params.curve2.y * c.w;
    return n / d - params.curve2.x / params.curve2.y;
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

// exposure, bloom, the map's tone curve on min(c × 2.8, white) over the curve at white, sRGB, screen bloom over that,
// then the lookup table in gamma space; written linear, for the sRGB target to encode again
@fragment
fn present(in: Out) -> @location(0) vec4<f32> {
    let e = params.b.w * adapted();
    var c = textureSampleLevel(input, linear_clamp, in.uv, 0.0).rgb * e;
    var bloom = vec3<f32>(0.0);
    if params.lut.w > 0.5 {
        bloom = textureSampleLevel(lower, linear_clamp, in.uv, 0.0).rgb;
    }
    c = c * (1.0 - params.bloom.w) + bloom * (params.bloom.z * e);
    c += bloom * params.bloom.x;
    c = curve(min(c * 2.8, vec3<f32>(params.curve2.z))) * params.curve2.w;
    c = to_srgb(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    let s = bloom * e;
    let screen = clamp(s / (s + 0.187) * 1.035, vec3<f32>(0.0), vec3<f32>(1.0)) * params.bloom.y;
    c = c + screen - c * screen;
    if params.lut.x > 0.0 {
        c = mix(c, textureSampleLevel(lut, linear_clamp, c * params.lut.y + params.lut.z, 0.0).rgb, params.lut.x);
    }
    return vec4<f32>(to_linear(c), 1.0);
}
