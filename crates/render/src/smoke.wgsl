// The composite pass: the scene's colour plus the smoke, raymarched per pixel through the cloud's volume and stopped at
// the scene's depth, so walls and floors cut the cloud where they are. A port of show-me-the-nades'
// web/app/src/scene/render/composite.ts, which draws the smoke as CS2's own shader does (smoke_volume's full-quality
// compute path, decompiled from the game's files, with the values the game's material gives it; its D25). It works in
// Hammer units with z up, as the cloud is stored; the camera's rays are turned into that frame.
//
// The pass blends into the frame's HDR target with a constant weight: 1 starts the picture over, 1/(n+1) averages the
// n-th jittered frame in, so a still view converges to a clean image.

struct Smoke {
    // clip space (reversed depth) back to Hammer units
    inverse_view_proj: mat4x4<f32>,
    // xyz the eye (Hammer units), w 1 while the smoke shows
    eye: vec4<f32>,
    // the camera's right, up and back axes in Hammer units
    right: vec4<f32>,
    up: vec4<f32>,
    back: vec4<f32>,
    // xyz the grid's minimum corner, w its size (32 voxels of 20 units)
    origin: vec4<f32>,
    // the box around everything the cloud fills; w: seconds after detonation, the fade
    box_min: vec4<f32>,
    box_max: vec4<f32>,
    // xyz towards the sun, w the frame number (the jitter's sequence)
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    sky_color: vec4<f32>,
    // the target's width and height in pixels
    screen: vec4<f32>,
}

@group(0) @binding(0) var<uniform> smoke: Smoke;
@group(0) @binding(1) var volume: texture_3d<f32>;
@group(0) @binding(2) var noise: texture_3d<f32>;
@group(0) @binding(3) var clamped: sampler;
@group(0) @binding(4) var repeated: sampler;
@group(1) @binding(0) var scene_color: texture_2d<f32>;
@group(1) @binding(1) var scene_depth: texture_depth_2d;

// smoke_volume's settings as the game's material sets them
const RAY_STEP: f32 = 6.0;
const DENSITY: f32 = 0.85;
const AMBIENT: f32 = 0.6;
const DIRECT: f32 = 0.35;
const INTERIOR_DARKENING: f32 = 0.5;
const RIM_LIGHT: f32 = 0.25;
const AO_DISTANCE: f32 = 16.0;
const AO_STRENGTH: f32 = 0.06;
const BOIL: f32 = -0.3;
const NOISE_SPEED: f32 = 1.5;
const NOISE_SCALE_LOW: f32 = 3.0;
const NOISE_SCALE_HIGH: f32 = 7.0;
const NOISE_STRENGTH_LOW: f32 = 0.5;
const NOISE_STRENGTH_HIGH: f32 = 0.35;
const NOISE_OFFSET: f32 = -0.95;
const NOISE_MAX: f32 = 1.05;
const NOISE_NORMAL: f32 = 0.5;
const NOISE_NORMAL_DIRECT: f32 = 0.5;
const VOLUME_NORMAL_BLEND: f32 = 0.5;
const SMOKE_NOISE: f32 = 0.8;

struct Out {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn fullscreen(@builtin(vertex_index) v: u32) -> Out {
    let uv = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u));
    return Out(vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0));
}

// interleaved gradient noise: a per-pixel offset that turns banding into fine grain
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

fn box_hit(ro: vec3<f32>, rd: vec3<f32>) -> vec2<f32> {
    let inv = 1.0 / rd;
    let a = (smoke.box_min.xyz - ro) * inv;
    let b = (smoke.box_max.xyz - ro) * inv;
    let lo = min(a, b);
    let hi = max(a, b);
    return vec2<f32>(max(max(lo.x, lo.y), lo.z), min(min(hi.x, hi.y), hi.z));
}

// the cloud's density at a grid coordinate (0..1 across the voxels), grown in behind its front
fn cloud_at(uvw: vec3<f32>) -> f32 {
    let v = textureSampleLevel(volume, clamped, uvw, 0.0);
    let arrival = v.g * (255.0 / 40.0);
    return v.r * smoothstep(arrival, arrival + 0.5, smoke.box_min.w);
}

// the game's noise as smoke_volume reads it: R + 0.95 G, times NoiseMax, times 4.6
fn noise_at(c: vec3<f32>) -> f32 {
    return textureSampleLevel(noise, repeated, c * 0.07, 0.0).r * NOISE_MAX * 4.6;
}

// saturation scaled about the colour's own maximum, as smoke_volume lifts its probe light (HSV, s × 1.1)
fn saturate1(c: vec3<f32>, k: f32) -> vec3<f32> {
    let hi = max(c.r, max(c.g, c.b));
    let lo = min(c.r, min(c.g, c.b));
    if hi <= 0.0 || hi == lo {
        return c;
    }
    let s = (hi - lo) / hi;
    let s2 = clamp(s * k, 0.0, 1.0);
    return hi - (hi - c) * (s2 / s);
}

fn world_at(ndc: vec2<f32>, depth: f32) -> vec3<f32> {
    let p = smoke.inverse_view_proj * vec4<f32>(ndc, depth, 1.0);
    return p.xyz / p.w;
}

@fragment
fn composite(in: Out) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.clip.xy);
    var color = textureLoad(scene_color, pixel, 0).rgb;
    if smoke.eye.w < 0.5 {
        return vec4<f32>(color, 1.0);
    }
    let uv = in.clip.xy / smoke.screen.xy;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let ro = smoke.eye.xyz;
    let rd = normalize(world_at(ndc, 0.5) - ro);
    let dz = textureLoad(scene_depth, pixel, 0);
    var t_scene = 1e20;
    // reversed depth: 0 is the far plane, where nothing was drawn
    if dz > 0.0 {
        t_scene = distance(world_at(ndc, dz), ro);
    }
    let hit = box_hit(ro, rd);
    let t0 = max(hit.x, 0.0);
    let t1 = min(hit.y, t_scene);
    if t1 <= t0 {
        return vec4<f32>(color, 1.0);
    }
    // the camera's axes: smoke_volume works its normals out across the screen, then turns them into the world
    let right = normalize(smoke.right.xyz);
    let up = normalize(smoke.up.xyz);
    let view = mat3x3<f32>(right, up, normalize(smoke.back.xyz));
    let forward = -view[2];
    let size = smoke.origin.w;
    let centre = smoke.origin.xyz + vec3<f32>(size * 0.5);
    let seconds = smoke.box_min.w;
    let fade = smoke.box_max.w;
    let time = seconds * NOISE_SPEED;
    let half_time = time * 0.5;
    let scroll = vec3<f32>(2.0, 2.0, 4.5) * time * 0.1;
    let stride = RAY_STEP * 2.0;
    var t = max(t0, 4.0) + stride * fract(ign(in.clip.xy) + smoke.sun_dir.w * 0.61803399) * SMOKE_NOISE;
    // as the smoke goes, its outside goes first
    let life = clamp(fade, 1e-4, 1.0);
    let inner = smoothstep(0.0, 0.8, life);
    let outer = smoothstep(0.2, 1.0, life);
    let hold = clamp(fade * 8.0, 0.0, 1.0);
    var acc = vec4<f32>(0.0);
    var sun_sum = 0.0;
    var ao_sum = 0.0;
    for (var i = 0; i < 128; i++) {
        if t > t1 || acc.a > 0.991 {
            break;
        }
        let p = ro + rd * t;
        let uvw = (p - smoke.origin.xyz) / size;
        let w = cloud_at(uvw);
        if w > 0.01 {
            // the density's gradient across the screen, pointing out of the cloud
            let volume_normal = normalize(vec3<f32>(w - cloud_at(uvw + right * 0.03), cloud_at(uvw - up * 0.03) - w, 0.06));

            // the noise's coordinates: the cloud's own, swirling slowly about z and wobbling
            let c = uvw - 0.5;
            let q = c * 7.0;
            let swirl = time * 0.04 + (0.2 + (sin(q.z * 5.0) + 0.5) * 0.15) * sin(half_time + 0.5) * sin(time * 0.187 + 0.5) * 0.2;
            let sw = sin(swirl);
            let cw = cos(swirl);
            let xy = vec2<f32>(q.x * cw - q.y * sw, q.x * sw + q.y * cw);
            let wobble = time + sin(half_time) * 0.02;
            let xz = vec2<f32>(xy.x, q.z) + vec2<f32>(sin(wobble + q.z * 2.7), cos(wobble + xy.x * 2.7)) * 0.05;
            let m = vec3<f32>(xz.x, xy.y, xz.y + (sin(xz.x * 3.0 + time * 0.35) + sin(xy.y * 2.84 + time * 0.235)) * 0.05);

            // low octave, mirrored about the centre and scrolling up, with its normal across the screen
            let low = m * NOISE_SCALE_LOW;
            let n_low = noise_at(abs(low) - scroll);
            let low_normal = normalize(vec3<f32>(n_low - noise_at(abs(low + right * 0.2) - scroll), n_low - noise_at(abs(low + up * 0.2) - scroll), 0.8 / NOISE_SCALE_LOW));
            // high octave, boiled along the normals, fading in past 200 units
            var h = q + view * (low_normal + volume_normal) * pow(max(n_low, 1e-4), 0.1) * BOIL * 0.2 + vec3<f32>(2.0, 2.0, 4.5) * ((n_low - 1.0) * 0.2) * BOIL * c.z;
            h.x += sin(m.z + time * 0.25) * 0.05;
            let high = h * NOISE_SCALE_HIGH;
            let n_high = noise_at(high);
            let high_normal = normalize(vec3<f32>(n_high - noise_at(high + right * 0.2), n_high - noise_at(high + up * 0.2), 0.8 / NOISE_SCALE_HIGH));
            let far = NOISE_STRENGTH_HIGH * dot(rd, forward) * clamp(distance(p, ro) * 0.005, 0.0, 1.0);
            let noise_value = mix(0.95, clamp(n_low, 0.0, 1.0), NOISE_STRENGTH_LOW) + mix(0.95, clamp(n_high, 0.0, 1.0), far) + NOISE_OFFSET;

            // density: thicker right at the camera, the outside going first as it fades, then eaten by the noise
            var d = clamp((w - 0.01) * 1.0101, 0.0, 1.0);
            d = clamp(d + (1.0 - clamp(distance(ro, p) * 0.1, 0.0, 1.0)) * d, 0.0, 1.0);
            let shape = vec3<f32>(c.xy, c.z * 1.2);
            var d0 = d;
            if life < 1.0 {
                if inner == outer {
                    d0 = d * life;
                } else {
                    d0 = d * (1.0 - smoothstep(outer, inner, clamp(length(shape), 0.0, 1.0)));
                }
            }
            let eaten = mix(d0 - (1.0 - noise_value), d0 + noise_value, d0) * hold;

            // the normal: outward from the cloud's middle, its gradient, and the noise's detail
            let outward = normalize(c + vec3<f32>(0.0, 0.0, 1e-4));
            let detail = vec3<f32>(low_normal.xy * NOISE_STRENGTH_LOW + high_normal.xy * mix(0.5, 1.0, clamp(n_low - 0.5, 0.0, 1.0)) * far, 0.0);
            let n = view * (mix(outward * view, volume_normal, VOLUME_NORMAL_BLEND) + detail * NOISE_NORMAL * mix(1.0, 2.0, clamp((200.0 - distance(p, centre)) * 0.005, 0.0, 1.0)));

            // the map's light where the cloud's outside faces this way
            let probe = textureSampleLevel(volume, clamped, clamp(mix(c * dot(outward, n), normalize(n + vec3<f32>(0.0, 0.0, 0.5)) * length(c), vec3<f32>(0.2)) + 0.5, vec3<f32>(0.03), vec3<f32>(0.97)), 0.0);
            let mu = dot(mix(outward, n, NOISE_NORMAL_DIRECT), smoke.sun_dir.xyz);
            let lit = (pow(clamp(mu * 0.8 + 0.2, 0.0, 1.0), 1.5) + pow(clamp(mu * 1.4 - 0.5, 0.0, 1.0), 3.0)) * probe.a;
            var ambient = saturate1(mix(0.3, 1.0, probe.b) * (smoke.sky_color.rgb + smoke.sun_color.rgb * 0.15), 1.1);
            ambient = normalize(ambient + 0.001) * min(length(ambient), 4.0);
            let col = ambient * clamp(1.0 - min(0.25, eaten), 0.0, 1.0) * clamp(1.0 - (d0 * 2.4 - eaten) * INTERIOR_DARKENING, 0.0, 1.0) * noise_value * (0.75 + lit * 0.25) * (1.0 + n.z * 0.5) * AMBIENT
                + smoke.sun_color.rgb * (0.5 * lit) * DIRECT;
            // smoke within AO_DISTANCE of the surface behind it darkens it
            let ao = smoothstep(0.0, 0.2, eaten + 0.3) * clamp((AO_DISTANCE - max(0.0, t_scene - t)) / AO_DISTANCE, 0.0, 1.0) * d0 * 6.0 * AO_STRENGTH;
            let a = smoothstep(0.0, 0.2 / (DENSITY * mix(0.5, 2.0, acc.a)), eaten);
            let layer = vec4<f32>(col * a, a);
            for (var k = 0; k < i32(RAY_STEP * 0.5); k++) {
                acc += layer * (1.0 - acc.a);
                sun_sum += lit;
                ao_sum += ao;
            }
        }
        t += stride;
    }
    if acc.a > 1e-5 {
        let edge = clamp(ao_sum - acc.a * 0.2, 0.0, 1.0);
        acc = vec4<f32>(acc.rgb * (1.0 - edge), min(1.0, acc.a + edge));
        // a rim of sunlight on the thin edges toward the sun
        let along = pow(clamp(dot(rd, smoke.sun_dir.xyz), 0.0, 1.0), 4.0) * 0.25;
        let sun2 = smoke.sun_color.rgb * smoke.sun_color.rgb;
        let lit_smoke = acc.rgb * (1.0 + sun2 * ((along + pow(along, 50.0) * 8.0) * (1.0 - sqrt(acc.a)) * acc.a) * sun_sum * RIM_LIGHT);
        color = color * (1.0 - clamp(acc.a, 0.0, 0.9999)) + lit_smoke;
    }
    return vec4<f32>(color, 1.0);
}
