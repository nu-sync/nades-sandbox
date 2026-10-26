// What a throw adds to the map, in Hammer units with z up (the database's frame; `view_proj` takes it to clip space):
// - meshes: the flight's tube and its marks, the grenade in flight, the lines to the players a flash blinded
//   (render/effects.ts in show-me-the-nades' web view);
// - glows: a flash's or HE's burst, a sprite that faces the camera and adds light;
// - the fire's sprites and the film and scorch under its flames (render/fire.ts), which read the scene's depth to fade
//   where they meet it (CS2's depth feathering).

struct Frame {
    // Hammer units to clip space
    view_proj: mat4x4<f32>,
    // Hammer units to the camera's (Hammer units, looking down −z), and on to clip space
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    // xyz the eye, w seconds after detonation
    eye: vec4<f32>,
    // xyz towards the sun, w its strength on the grenade
    sun: vec4<f32>,
    // the target's width and height in pixels; the near and far planes in metres (the depth buffer's)
    screen: vec4<f32>,
    // x 1 when the flipbooks play; y the film's row in the palette
    flip: vec4<f32>,
    // the first two flipbooks' columns and rows
    grid: vec4<f32>,
    // the third's
    grid2: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var fire_noise: texture_2d<f32>;
@group(0) @binding(2) var palette: texture_2d<f32>;
@group(0) @binding(3) var book0: texture_2d<f32>;
@group(0) @binding(4) var book1: texture_2d<f32>;
@group(0) @binding(5) var book2: texture_2d<f32>;
@group(0) @binding(6) var repeated: sampler;
@group(0) @binding(7) var clamped: sampler;
@group(1) @binding(0) var scene_depth: texture_depth_2d;

const INCH: f32 = 0.0254;

// how far the scene is from the eye at this pixel, in Hammer units along the view (three's perspectiveDepthToViewZ)
fn scene_distance(clip: vec4<f32>) -> f32 {
    let d = textureLoad(scene_depth, vec2<i32>(clip.xy), 0);
    let near = frame.screen.z;
    let far = frame.screen.w;
    return near * far / (d * (far - near) + near) / INCH;
}

// --- meshes ---

struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    // x 1 when lit by the sun (the grenade), y 1 when it fades out near the eye (the tube under a chasing camera)
    @location(3) params: vec2<f32>,
}

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) params: vec2<f32>,
    @location(3) depth: f32,
}

@vertex
fn mesh_vertex(in: MeshIn) -> MeshOut {
    let view = frame.view * vec4<f32>(in.position, 1.0);
    return MeshOut(frame.view_proj * vec4<f32>(in.position, 1.0), in.color, in.normal, in.params, -view.z);
}

@fragment
fn mesh(in: MeshOut) -> @location(0) vec4<f32> {
    var rgb = in.color.rgb;
    if in.params.x > 0.5 {
        // a little lit object: the sun on its facing side over a dim sky
        let n = normalize(in.normal);
        rgb = rgb * (max(dot(n, frame.sun.xyz), 0.0) * frame.sun.w + 0.35);
    }
    var alpha = in.color.a;
    if in.params.y > 0.5 {
        alpha *= smoothstep(60.0, 260.0, in.depth);
    }
    return vec4<f32>(rgb, alpha);
}

// --- glows: a flash's or HE's burst (a THREE.Sprite with a radial gradient, added to the scene) ---

struct GlowIn {
    @builtin(vertex_index) corner: u32,
    // xyz the centre, w the sprite's size
    @location(0) centre: vec4<f32>,
    // rgb its colour, a its opacity
    @location(1) color: vec4<f32>,
}

struct GlowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) quad: vec2<f32>,
    @location(1) color: vec4<f32>,
}

fn corner_of(k: u32) -> vec2<f32> {
    // two triangles: (−1,−1) (1,−1) (1,1), (−1,−1) (1,1) (−1,1)
    var corners = array<vec2<f32>, 6>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    return corners[k];
}

@vertex
fn glow_vertex(in: GlowIn) -> GlowOut {
    let q = corner_of(in.corner);
    var view = frame.view * vec4<f32>(in.centre.xyz, 1.0);
    view = vec4<f32>(view.xy + q * in.centre.w * 0.5, view.zw);
    return GlowOut(frame.projection * view, q, in.color);
}

@fragment
fn glow(in: GlowOut) -> @location(0) vec4<f32> {
    // the canvas's radial gradient: opaque at the centre, 0.45 at 45% out, clear at the edge
    let r = length(in.quad);
    let a = select(mix(0.45, 0.0, (r - 0.45) / 0.55), mix(1.0, 0.45, r / 0.45), r < 0.45) * f32(r < 1.0);
    return vec4<f32>(in.color.rgb, a * in.color.a);
}

// --- the fire's sprites ---

struct SpriteIn {
    @builtin(vertex_index) corner: u32,
    @location(0) pos: vec4<f32>,
    // half-width, half-height, roll, seed
    @location(1) shape: vec4<f32>,
    // alpha, age, the gradient's row, what it is (0 a flame, 1 steam, 2 a fleck)
    @location(2) look: vec4<f32>,
    // linear colour × overbright, and whether it covers
    @location(3) colour: vec4<f32>,
    // level, desaturation
    @location(4) tone: vec4<f32>,
    // the flipbook's slot (−1 none), its sequence's first frame and count, the frame-rate jitter
    @location(5) book: vec4<f32>,
}

struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) quad: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) look: vec4<f32>,
    @location(3) colour: vec4<f32>,
    @location(4) tone: vec2<f32>,
    @location(5) seed: f32,
    @location(6) distance: f32,
    @location(7) book_uv: vec2<f32>,
    @location(8) book_frame: vec4<f32>,
}

@vertex
fn sprite_vertex(in: SpriteIn) -> SpriteOut {
    let position = corner_of(in.corner);
    var view = frame.view * vec4<f32>(in.pos.xyz, 1.0);
    // world up as the screen sees it, so flames stand up
    let up = (frame.view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz;
    let level = length(up.xy);
    let y = normalize(mix(vec2<f32>(0.0, 1.0), up.xy, smoothstep(0.05, 0.3, level)) + vec2<f32>(0.0, 1e-5));
    let x = vec2<f32>(y.y, -y.x);
    let radius = in.shape.x;
    // an ellipsoid of this half-width and half-height, seen from this elevation: flat from the side, round from above
    let tall = sqrt(in.shape.y * in.shape.y * level * level + radius * radius * (1.0 - level * level));
    view = vec4<f32>(view.xy + x * position.x * radius + y * position.y * tall, view.zw);
    var out: SpriteOut;
    out.clip = frame.projection * view;
    let c = cos(in.shape.z);
    let s = sin(in.shape.z);
    let roll = mat2x2<f32>(vec2<f32>(c, s), vec2<f32>(-s, c));
    // the noise keeps its grain however flat the sprite is
    out.uv = roll * vec2<f32>(position.x, position.y * tall / radius) * clamp(radius / 30.0, 0.5, 2.0);
    out.quad = position;
    out.seed = in.shape.w;
    out.distance = -view.z;
    // CS2 fades a sprite as it grows from 45% to 57.5% of the screen
    let screen = radius * frame.projection[1][1] / max(-view.z, 1.0);
    out.look = vec4<f32>(in.look.x * (1.0 - smoothstep(0.45, 0.575, screen)), in.look.yzw);
    out.colour = in.colour;
    out.tone = in.tone.xy;
    // the flipbook: the frames either side of where its life is, and how far between them. The flames fill about the
    // middle of their frames (x 0.15–0.85, y 0.1–0.85 up from the bottom): that box is stretched over the sprite
    let r = roll * position;
    out.book_uv = vec2<f32>(0.15, 0.1) + (r * 0.5 + 0.5) * vec2<f32>(0.7, 0.75);
    let f = clamp(in.look.y * in.book.w, 0.0, 1.0) * max(in.book.z - 1.0, 0.0);
    let first = floor(f);
    out.book_frame = vec4<f32>(select(-1.0, in.book.x, frame.flip.x > 0.5), in.book.y + first, in.book.y + min(first + 1.0, in.book.z - 1.0), f - first);
    return out;
}

// one frame of a flipbook atlas, frames row by row from its top, with its derivatives
fn cell(book: u32, grid: vec2<f32>, index: f32, q: vec2<f32>, dq: vec4<f32>) -> vec4<f32> {
    let row = floor(index / grid.x);
    let uv = (vec2<f32>(index - row * grid.x, row) + vec2<f32>(q.x, 1.0 - q.y)) / grid;
    let dx = vec2<f32>(dq.x, -dq.y) / grid;
    let dy = vec2<f32>(dq.z, -dq.w) / grid;
    if book == 0u {
        return textureSampleGrad(book0, clamped, uv, dx, dy);
    }
    if book == 1u {
        return textureSampleGrad(book1, clamped, uv, dx, dy);
    }
    return textureSampleGrad(book2, clamped, uv, dx, dy);
}

fn noise4(uv: vec2<f32>, d: vec4<f32>, k: f32) -> vec4<f32> {
    return textureSampleGrad(fire_noise, repeated, uv, d.xy * k, d.zw * k);
}

fn gradient_at(level: f32, row: f32) -> vec3<f32> {
    return pow(textureSampleLevel(palette, clamped, vec2<f32>(level, row), 0.0).rgb, vec3<f32>(2.2));
}

@fragment
fn sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    // the derivatives, taken before any branch
    let duv = vec4<f32>(dpdx(in.uv), dpdy(in.uv));
    let dbook = vec4<f32>(dpdx(in.book_uv), dpdy(in.book_uv));
    let d = length(in.quad);
    let book = in.look.w < 0.5 && in.book_frame.x > -0.5;
    if !book && d >= 1.0 {
        discard;
    }
    let age = in.look.y;
    let o = vec2<f32>(in.seed * 7.31, in.seed * 3.17);
    let feather = in.look.x * clamp((scene_distance(in.clip) - in.distance) / 6.0, 0.0, 1.0);
    var colour: vec3<f32>;
    var a: f32;
    if book {
        if any(in.book_uv < vec2<f32>(0.0)) || any(in.book_uv > vec2<f32>(1.0)) {
            discard;
        }
        let slot = u32(in.book_frame.x + 0.5);
        var grid = frame.grid.xy;
        if slot == 1u {
            grid = frame.grid.zw;
        } else if slot == 2u {
            grid = frame.grid2.xy;
        }
        let t = mix(cell(slot, grid, in.book_frame.y, in.book_uv, dbook), cell(slot, grid, in.book_frame.z, in.book_uv, dbook), in.book_frame.w);
        // the texel's brightness picks the gradient's colour, half and half with its own (flTextureBlend 0.5)
        let level = clamp(max(t.r, max(t.g, t.b)) * in.tone.x, 0.0, 1.0);
        let own = pow(t.rgb, vec3<f32>(2.2));
        colour = mix(own, gradient_at(level, in.look.z), 0.5);
        colour = mix(colour, vec3<f32>(dot(colour, vec3<f32>(0.2126, 0.7152, 0.0722))), in.tone.y);
        // a texel covers what is behind it only as far as it is bright
        a = t.a;
        return vec4<f32>(colour * in.colour.rgb * a, a * in.colour.a * level) * feather;
    }
    if in.look.w < 0.5 {
        // a flame: a blob that churns upward through the flipbook and dims as it burns out
        let q = in.uv * 0.36 + o;
        let warp = noise4(q * 0.8 + vec2<f32>(0.0, -0.3 * age), duv, 0.36 * 0.8).g - 0.5;
        let n = noise4(q + vec2<f32>(0.35 * warp, -0.5 * age), duv, 0.36).r * 0.62 + noise4(q * 2.3 + vec2<f32>(-0.25 * warp, -1.0 * age), duv, 0.36 * 2.3).b * 0.38;
        let grain = noise4(q * 3.1 + vec2<f32>(0.0, -1.7 * age), duv, 0.36 * 3.1).a;
        // a ragged edge, so the fringe breaks into flecks rather than a rim
        let body = 1.0 - smoothstep(0.15, 1.0, length(in.quad - vec2<f32>(0.0, 0.25 * age - 0.1)) + (n - 0.5) * 1.4 + (grain - 0.5) * 0.5);
        // the body sits high (peach to white) and only a thin fringe passes through the gradient's dim colours
        let level = clamp(pow(body, 0.35) * (0.62 + 0.5 * n) * mix(1.1, 0.72, age) * in.tone.x, 0.0, 1.0);
        a = smoothstep(0.0, 0.9, body) * (0.5 + 0.5 * n);
        colour = gradient_at(level, in.look.z);
        colour = mix(colour, vec3<f32>(dot(colour, vec3<f32>(0.2126, 0.7152, 0.0722))), in.tone.y);
    } else if in.look.w < 1.5 {
        // steam: a soft, broken wisp
        let n = noise4(in.uv * 0.3 + o + vec2<f32>(0.0, -0.2 * age), duv, 0.3).r;
        a = (1.0 - smoothstep(0.25, 1.0, d + (n - 0.5) * 0.9)) * (0.5 + 0.7 * n);
        colour = vec3<f32>(1.0);
    } else {
        // a glowing fleck
        a = 1.0 - smoothstep(0.0, 1.0, d);
        a *= a;
        colour = vec3<f32>(1.0);
    }
    a *= feather;
    return vec4<f32>(colour * in.colour.rgb * a, a * in.colour.a);
}

// --- the ground under each flame: the burning film and the scorch ---

struct PatchIn {
    @builtin(vertex_index) corner: u32,
    @location(0) pos: vec4<f32>,
    @location(1) normal: vec4<f32>,
    // radius, glow, scorch, seed
    @location(2) patch_data: vec4<f32>,
}

struct PatchOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) quad: vec2<f32>,
    @location(1) patch_data: vec4<f32>,
    @location(2) distance: f32,
}

@vertex
fn patch_vertex(in: PatchIn) -> PatchOut {
    let position = corner_of(in.corner);
    let n = normalize(in.normal.xyz);
    let t = normalize(select(cross(n, vec3<f32>(1.0, 0.0, 0.0)), cross(n, vec3<f32>(0.0, 0.0, 1.0)), abs(n.z) < 0.9));
    let b = cross(n, t);
    let world = in.pos.xyz + n * 2.0 + (t * position.x + b * position.y) * in.patch_data.x;
    let view = frame.view * vec4<f32>(world, 1.0);
    return PatchOut(frame.view_proj * vec4<f32>(world, 1.0), position, in.patch_data, -view.z);
}

@fragment
fn fire_patch(in: PatchOut) -> @location(0) vec4<f32> {
    let dq = vec4<f32>(dpdx(in.quad), dpdy(in.quad));
    let d = length(in.quad);
    if d >= 1.0 {
        discard;
    }
    let o = vec2<f32>(in.patch_data.w * 5.1, in.patch_data.w * 9.7);
    let n = noise4(in.quad * 0.35 + o, dq, 0.35).r;
    // only where it lies on a surface: a projected decal, not a card over a drop
    let mask = (1.0 - smoothstep(0.35, 1.0, d + (n - 0.5) * 0.6)) * (1.0 - smoothstep(5.0, 16.0, abs(scene_distance(in.clip) - in.distance)));
    let time = frame.eye.w;
    let flicker = noise4(in.quad * 0.55 + o + vec2<f32>(time * 0.31, -time * 0.23), dq, 0.55).g;
    // a film of burning fuel: deep red to orange, the white of the flames above it
    let level = clamp(mask * (0.15 + 0.6 * flicker), 0.0, 1.0);
    let film = gradient_at(level, frame.flip.y) * in.patch_data.y * mask * 0.3;
    let scorch = in.patch_data.z * mask * (0.55 + 0.5 * noise4(in.quad * 0.9 + o.yx, dq, 0.9).b) * 0.8;
    return vec4<f32>(film, clamp(scorch, 0.0, 0.85));
}
