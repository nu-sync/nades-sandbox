enable wgpu_binding_array;

// The map and its 3D skybox in CS2's look (crates/render/src/map.rs). Vertices are pulled from the pack's Geometry
// section (crates/pack/src/scene.rs): each instance's slot says where its draw's streams start and which material it
// has. Every texture is in one binding array, which a material indexes.
//
// Light is CS2's, as Source 2 Viewer ports it for lightmap version 2 (Renderer/Shaders/common/lighting.slang, pbr.slang):
// diffuse = albedo × (sun × N·L × (1 − baked shadow) + indirect) × occlusion, the indirect light from the irradiance
// lightmap shaded through the directional lightmap; specular is GGX for the sun and the sky's reflection, normalised by
// the irradiance here. Materials follow csgo_environment(_blend), csgo_lightmappedgeneric and csgo_effects as
// show-me-the-nades' web view ports them (render/csgoMaterials.ts, csgoEffects.ts), with the game's texture channels.
// Fog is CS2's gradient and cubemap fog (render/fog.ts).

const NONE: u32 = 0xffffffffu;
// what a frame shows: 0 the picture; for debugging (NADES_DEBUG), 1 draws without lightmap coordinates in magenta,
// 2 the indirect light alone, 3 the sun's visibility, 4 the albedo
override debug_mode: u32 = 0u;
const PI: f32 = 3.14159265;

struct Frame {
    view_proj: mat4x4<f32>,
    // clip back to this pass's space, for the sky's rays
    inverse_view_proj: mat4x4<f32>,
    // xyz the eye in this pass's space, w seconds (effects' masks pan with it)
    eye: vec4<f32>,
    // xyz towards the sun, w 1 when there is a sun
    sun_dir: vec4<f32>,
    // rgb the sun's colour × brightness
    sun_color: vec4<f32>,
    // irradiance, directional irradiance, sun shadows (texture indices or NONE), w: 1 for bicubic shadows
    lightmaps: vec4<u32>,
    // rgb the sky's tint, w its yaw (radians)
    sky: vec4<f32>,
    // x 1 with a sky, y its blurriest mip for the fog, z the ambient's mip, w the shadow mask's width
    sky_info: vec4<f32>,
    // gradient fog: distance bias, height bias, distance scale, height scale
    fog_gradient: vec4<f32>,
    // gradient fog: distance and height exponents
    fog_gradient_exp: vec4<f32>,
    // gradient fog: colour × strength, its most opacity (0: off)
    fog_gradient_color: vec4<f32>,
    // cubemap fog: distance offset, distance scale, mip bias, falloff exponent
    fog_cube: vec4<f32>,
    // cubemap fog: height offset, height scale, height exponent, the blurriest mip
    fog_cube_height: vec4<f32>,
    // cubemap fog: the sky's scale, the most opacity (0: off)
    fog_cube_color: vec4<f32>,
    // the target's width and height in pixels, the camera's near and far planes (for depth feathering)
    screen: vec4<f32>,
    // which channel of the baked shadow mask is the sun's (one-hot; zero for none)
    shadow_mask: vec4<f32>,
    // x the light probe volumes this pass samples (0 for none)
    probe_info: vec4<f32>,
}

// one light probe volume (crates/pack ProbeVolume; Source 2 Viewer's LightProbeVolume)
struct ProbeVolume {
    to_local: mat4x4<f32>,
    atlas_scale: vec4<f32>,
    atlas_offset: vec4<f32>,
    border_min: vec4<f32>,
    border_max: vec4<f32>,
    inverse_edge: vec4<f32>,
    bounds_extend: vec4<f32>,
}

struct Slot {
    world: mat4x4<f32>,
    // the world matrix's inverse transpose, for normals and tangents (three columns)
    normal_matrix: mat3x3<f32>,
    // word offsets into the Geometry section, NONE for an absent stream
    positions: u32,
    uv0: u32,
    uv1: u32,
    lightmap: u32,
    frames: u32,
    colors: u32,
    paint: u32,
    material: u32,
}

struct Material {
    // base colour (layer 1 colour), normal (layer 1), occlusion, layer 1 height
    tex0: vec4<u32>,
    // layer 2 colour, normal, height; layer 3 colour
    tex1: vec4<u32>,
    // layer 3 normal, height; the two-layer material's second colour and modulation
    tex2: vec4<u32>,
    // x kind (0 generic, 1 environment, 2 two-layer, 3 effects, 4 unlit, 5 black), y layers, z flags, w alpha cutoff (bits)
    info: vec4<u32>,
    factor: vec4<f32>,
    // environment: the object's tint, its amount. two-layer: layer 1's tint, the blend softness
    tint: vec4<f32>,
    // two-layer: layer 2's tint, F_FANCY_BLENDING
    tint2: vec4<f32>,
    // per layer, two rows of its coordinates' affine transform
    uv: array<vec4<f32>, 6>,
    // per layer, three rows of each colour matrix
    adjust: array<vec4<f32>, 9>,
    tint_adjust: array<vec4<f32>, 9>,
    // per layer: tint mask contrast, brightness, height scale, height zero point
    layer_a: array<vec4<f32>, 3>,
    // per layer: blend softness, underlying height influence, mask with height, normal contrast
    layer_b: array<vec4<f32>, 3>,
    // per layer: roughness contrast, brightness, metalness on, vertex colour mode
    layer_c: array<vec4<f32>, 3>,
    // effects: fade distance, falloff, min, max
    fade: vec4<f32>,
    // effects: fresnel exponent, falloff, min, max
    fresnel: vec4<f32>,
    // effects: feather distance, falloff, colour boost, opacity
    effects: vec4<f32>,
    // effects: per mask its scale and pan speed
    masks: array<vec4<f32>, 3>,
    // layers 2 and 3: the direction a layer keeps to (scene frame; w 1 when it does)
    facing: array<vec4<f32>, 2>,
    // layer 2's smoothstep band, then layer 3's
    facing_range: vec4<f32>,
}

// info.z
const MODEL_TINT_1: u32 = 1u;
const NEW_BLENDING: u32 = 8u;
const UV2_1: u32 = 16u;
const RGB_NORMAL_1: u32 = 128u;
const ADDITIVE: u32 = 1024u;
const MASK_1: u32 = 2048u;
const DOUBLE_SIDED: u32 = 16384u;

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<storage, read> slots: array<Slot>;
@group(1) @binding(1) var<storage, read> geometry: array<u32>;
@group(1) @binding(2) var<storage, read> materials: array<Material>;
@group(1) @binding(3) var textures: binding_array<texture_2d<f32>>;
@group(1) @binding(4) var sky_cube: texture_cube<f32>;
@group(1) @binding(5) var material_sampler: sampler;
@group(1) @binding(6) var clamp_sampler: sampler;
@group(1) @binding(7) var noise: texture_2d<f32>;
@group(1) @binding(8) var probe_irradiance: texture_3d<f32>;
@group(1) @binding(9) var probe_shadows: texture_3d<f32>;
@group(1) @binding(10) var<storage, read> probe_volumes: array<ProbeVolume>;
// effects only: the depth of what was drawn before, for feathering
@group(2) @binding(0) var scene_depth: texture_depth_2d;

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec4<f32>,
    @location(4) lightmap_uv: vec2<f32>,
    @location(5) color: vec4<f32>,
    @location(6) paint: vec4<f32>,
    @location(7) @interpolate(flat) material: u32,
    @location(8) @interpolate(flat) lit: u32,
}

fn pair(word: u32) -> vec2<f32> {
    return vec2<f32>(bitcast<f32>(geometry[word]), bitcast<f32>(geometry[word + 1u]));
}

@vertex
fn vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> Varyings {
    let slot = slots[i];
    let xy = unpack2x16snorm(geometry[slot.positions + 2u * v]);
    let z = unpack2x16snorm(geometry[slot.positions + 2u * v + 1u]).x;
    let world = slot.world * vec4<f32>(xy, z, 1.0);
    var out: Varyings;
    out.clip = frame.view_proj * world;
    out.world = world.xyz;
    let normal = unpack4x8snorm(geometry[slot.frames + 2u * v]);
    let tangent = unpack4x8snorm(geometry[slot.frames + 2u * v + 1u]);
    out.normal = slot.normal_matrix * normal.xyz;
    // a tangent turns like a position's direction, its handedness kept
    out.tangent = vec4<f32>((slot.world * vec4<f32>(tangent.xyz, 0.0)).xyz, select(1.0, -1.0, tangent.w < 0.0));
    let uv0 = pair(slot.uv0 + 2u * v);
    var uv1 = uv0;
    if slot.uv1 != NONE {
        uv1 = pair(slot.uv1 + 2u * v);
    }
    out.uv = vec4<f32>(uv0, uv1);
    out.lightmap_uv = vec2<f32>(0.0);
    out.lit = 0u;
    if slot.lightmap != NONE {
        out.lightmap_uv = pair(slot.lightmap + 2u * v);
        out.lit = 1u;
    }
    out.color = vec4<f32>(1.0);
    if slot.colors != NONE {
        out.color = unpack4x8unorm(geometry[slot.colors + v]);
    }
    out.paint = vec4<f32>(0.0);
    if slot.paint != NONE {
        let at = slot.paint + 4u * v;
        out.paint = vec4<f32>(bitcast<f32>(geometry[at]), bitcast<f32>(geometry[at + 1u]), bitcast<f32>(geometry[at + 2u]), bitcast<f32>(geometry[at + 3u]));
    }
    out.material = slot.material;
    return out;
}

// --- sampling: every texture through explicit gradients, so a material may sample in any branch ---

struct Uv {
    uv: vec2<f32>,
    dx: vec2<f32>,
    dy: vec2<f32>,
}

fn sample(t: u32, at: Uv, default_value: vec4<f32>) -> vec4<f32> {
    if t == NONE {
        return default_value;
    }
    return textureSampleGrad(textures[t], material_sampler, at.uv, at.dx, at.dy);
}

// a layer's coordinates: its UV set through its affine transform, the gradients through its linear part
fn layer_uv(m: Material, layer: u32, set2: bool, uv0: Uv, uv1: Uv) -> Uv {
    var base = uv0;
    if set2 {
        base = uv1;
    }
    let r0 = m.uv[layer * 2u];
    let r1 = m.uv[layer * 2u + 1u];
    let linear = mat2x2<f32>(vec2<f32>(r0.x, r1.x), vec2<f32>(r0.y, r1.y));
    return Uv(linear * base.uv + vec2<f32>(r0.z, r1.z), linear * base.dx, linear * base.dy);
}

// --- colour ---

const LUMA: vec3<f32> = vec3<f32>(0.2125, 0.7154, 0.0721);

fn contrast_brightness(value: f32, contrast: f32, brightness: f32) -> f32 {
    return clamp(((value - 0.5) * contrast + 0.5) * brightness, 0.0, 1.0);
}

fn color_matrix(rows: array<vec4<f32>, 9>, layer: u32, c: vec3<f32>) -> vec3<f32> {
    let x = vec4<f32>(c, 1.0);
    return vec3<f32>(dot(rows[layer * 3u], x), dot(rows[layer * 3u + 1u], x), dot(rows[layer * 3u + 2u], x));
}

// the object's tint as a colour of the same luminance (ColorizeTint)
fn colorize(albedo: vec3<f32>, tint: vec3<f32>, amount: f32) -> vec3<f32> {
    let tint_norm = normalize(max(tint, vec3<f32>(0.001)));
    let luma = dot(albedo, LUMA);
    let tinted = min(luma / dot(tint_norm, LUMA), 3.0 * luma * max(max(tint.r, tint.g), tint.b));
    return clamp(mix(albedo, tint_norm * tinted, amount), vec3<f32>(0.0), vec3<f32>(1.0));
}

// a layer's colour: corrected (or not) and tinted by its own matrices through its tint mask, then colourized
fn layer_color(m: Material, layer: u32, color: vec3<f32>, tint_mask: f32, model_tint: bool) -> vec3<f32> {
    let corrected = color_matrix(m.adjust, layer, color);
    let tinted = mix(corrected, color_matrix(m.tint_adjust, layer, color), tint_mask);
    return colorize(tinted, m.tint.rgb, m.tint.a * tint_mask * select(0.0, 1.0, model_tint));
}

// --- the layers' blend (csgo_environment_blend) ---

fn band_weight(difference: f32, bf: f32, raw_height: f32, softness_in: f32, mask_with_height: f32) -> f32 {
    let softness = max(softness_in, 0.0001);
    let quarter = softness * 0.25;
    let edge_denom = 0.02 + quarter;
    var crossfade = clamp(0.5 + difference / (2.0 * softness), 0.0, 1.0);
    let edge_hi = clamp((clamp(bf + difference * 0.1 + 0.2, 0.0, 1.0) - (0.98 - quarter)) / edge_denom, 0.0, 1.0);
    crossfade = mix(crossfade, 1.0, edge_hi);
    let edge_lo = clamp((edge_denom - clamp(bf + difference * 0.05 - 0.05, 0.0, 1.0)) / edge_denom, 0.0, 1.0);
    crossfade = mix(crossfade, 0.0, edge_lo);
    return clamp(crossfade * mix(0.5, raw_height, mask_with_height) * 2.0, 0.0, 1.0);
}

// the new blend: x the layer's weight, y the paint's signed factor, z that times the weight, w the height carried up
fn blend_layer(below: vec4<f32>, bf: f32, raw_height: f32, zero: f32, scale: f32, softness: f32, underlying: f32, mask_with_height: f32) -> vec4<f32> {
    if bf <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, below.w);
    }
    let signed_factor = 2.0 * bf - 1.0;
    let height_plus_signed = (raw_height - zero) * scale + signed_factor + max(below.y, 0.0);
    let difference = height_plus_signed - mix(below.z, below.w, underlying);
    let weight = band_weight(difference, bf, raw_height, softness, mask_with_height);
    var o = vec4<f32>(weight, signed_factor, 0.0, below.w);
    if weight > 0.05 {
        o.z = signed_factor * weight;
        o.w = mix(below.w, max(below.w, height_plus_signed), weight);
    }
    return o;
}

// how much of a layer's paint a surface keeps (F_BLEND_BY_FACING_DIRECTION_N): all of it when the layer does not
// face a direction, else more the more the surface's normal turns toward it
fn facing_mask(facing: vec4<f32>, band: vec2<f32>, normal: vec3<f32>) -> f32 {
    if facing.w == 0.0 {
        return 1.0;
    }
    return smoothstep(band.x, band.y, dot(facing.xyz, normal) * 0.5 + 0.5);
}

// the old blend: two heights weighed against each other around the paint
fn blend_weights(height_tex: vec2<f32>, height_scale: vec2<f32>, height_zero: vec2<f32>, blend_factor: f32, blend_softness: f32) -> vec2<f32> {
    let h1 = height_scale.x + blend_softness;
    let height1 = height_tex.x * h1;
    let h2 = height_scale.y + blend_softness;
    let h22 = height_tex.y * (height_scale.y - blend_softness);
    let blend1 = (-height_zero.x * h1 - ((1.0 - height_zero.y) * h2)) - blend_softness;
    let blend2 = (1.0 - height_zero.x) * h1 - ((-height_zero.y) * h2);
    let h2x = h22 + mix(blend1, blend2, blend_factor);
    var weights = max(vec2<f32>(height1, h2x) - vec2<f32>(max(height1, h2x) - blend_softness), vec2<f32>(0.0));
    weights.x += 0.001;
    return weights / (weights.x + weights.y);
}

// --- normals ---

// Source 2 Viewer's DecodeHemiOctahedronNormal: the game's normal maps, red and green
fn hemi_oct(e: vec2<f32>) -> vec3<f32> {
    let t = vec2<f32>(e.x + e.y - 1.003922, e.x - e.y);
    let n = normalize(vec3<f32>(t, 1.0 - abs(t.x) - abs(t.y)));
    return vec3<f32>(n.x, -n.y, n.z);
}

// a tangent-space normal from a normal map's texel, by its encoding, steered from flat by `contrast`
fn decode_normal(texel: vec4<f32>, rgb: bool, contrast: f32) -> vec3<f32> {
    var n: vec3<f32>;
    if rgb {
        n = normalize(texel.xyz * 2.0 - 1.0);
    } else {
        n = hemi_oct(texel.xy);
    }
    return normalize(mix(vec3<f32>(0.0, 0.0, 1.0), n, contrast));
}

// --- what a material gives the light ---

struct Surface {
    albedo: vec3<f32>,
    alpha: f32,
    // tangent space
    normal: vec3<f32>,
    roughness: f32,
    metalness: f32,
    occlusion: f32,
}

const FLAT: vec4<f32> = vec4<f32>(0.5, 0.5, 1.0, 1.0);
const NO_HEIGHT: vec4<f32> = vec4<f32>(0.5, 1.0, 1.0, 0.0);

fn environment(m: Material, in: Varyings, uv0: Uv, uv1: Uv) -> Surface {
    let flags = m.info.z;
    let layers = m.info.y;
    var s: Surface;
    let at1 = layer_uv(m, 0u, (flags & UV2_1) != 0u, uv0, uv1);
    let c1 = sample(m.tex0.x, at1, vec4<f32>(1.0));
    let h1 = sample(m.tex0.w, at1, NO_HEIGHT);
    let n1 = sample(m.tex0.y, at1, FLAT);
    let tint_mask1 = contrast_brightness(h1.g, m.layer_a[0].x, m.layer_a[0].y);
    var albedo = layer_color(m, 0u, c1.rgb, tint_mask1, (flags & MODEL_TINT_1) != 0u);
    var normal = decode_normal(n1, (flags & RGB_NORMAL_1) != 0u, m.layer_b[0].w);
    var roughness = contrast_brightness(select(1.0, n1.b, m.tex0.y != NONE && (flags & RGB_NORMAL_1) == 0u), m.layer_c[0].x, m.layer_c[0].y);
    var metalness = h1.a * m.layer_c[0].z;
    var occlusion = c1.a;
    var w2 = 0.0;
    var w3 = 0.0;
    var tint_mask2 = 0.0;
    var tint_mask3 = 0.0;
    if layers >= 2u {
        let at2 = layer_uv(m, 1u, (flags & (UV2_1 << 1u)) != 0u, uv0, uv1);
        let c2 = sample(m.tex1.x, at2, vec4<f32>(1.0));
        let h2 = sample(m.tex1.z, at2, NO_HEIGHT);
        let n2 = sample(m.tex1.y, at2, FLAT);
        tint_mask2 = contrast_brightness(h2.g, m.layer_a[1].x, m.layer_a[1].y);
        let albedo2 = layer_color(m, 1u, c2.rgb, tint_mask2, (flags & (MODEL_TINT_1 << 1u)) != 0u);
        // the paint, kept to the surfaces each layer faces; layer 3's softness reads its paint before that, as the
        // game's vertex shader does
        let surface_normal = normalize(in.normal);
        let paint2 = in.paint.x * facing_mask(m.facing[0], m.facing_range.xy, surface_normal);
        let paint3 = in.paint.y * facing_mask(m.facing[1], m.facing_range.zw, surface_normal);
        let bf2 = clamp(paint2 * 1.1 - 0.05, 0.0, 1.0);
        let bf3 = clamp(paint3 * 1.1 - 0.05, 0.0, 1.0);
        // the blend softness: the paint's own plus the material's (VertexPaintBlendParams)
        var base_softness = m.layer_b[1].x;
        if layers == 3u {
            if paint2 < 0.001 {
                base_softness = m.layer_b[2].x;
            }
            base_softness = mix(base_softness, m.layer_b[2].x, in.paint.y);
        }
        let softness = clamp(in.paint.w + base_softness, 0.001, 1.0);
        var carry2 = vec4<f32>(0.0);
        if (flags & NEW_BLENDING) != 0u {
            let carry1 = vec4<f32>(0.0, 0.0, 0.0, (h1.r - m.layer_a[0].w) * m.layer_a[0].z);
            carry2 = blend_layer(carry1, bf2, h2.r, m.layer_a[1].w, m.layer_a[1].z, softness, m.layer_b[1].y, m.layer_b[1].z);
            w2 = carry2.x;
        } else {
            w2 = blend_weights(vec2<f32>(h1.r - m.layer_a[0].w, h2.r - m.layer_a[1].w), vec2<f32>(m.layer_a[0].z, m.layer_a[1].z), vec2<f32>(m.layer_a[0].w, m.layer_a[1].w), paint2, softness).y;
        }
        albedo = mix(albedo, albedo2, w2);
        normal = mix(normal, decode_normal(n2, false, m.layer_b[1].w), w2);
        roughness = mix(roughness, contrast_brightness(select(1.0, n2.b, m.tex1.y != NONE), m.layer_c[1].x, m.layer_c[1].y), w2);
        metalness = mix(metalness, h2.a * m.layer_c[1].z, w2);
        occlusion = mix(occlusion, c2.a, w2);
        if layers == 3u {
            let at3 = layer_uv(m, 2u, (flags & (UV2_1 << 2u)) != 0u, uv0, uv1);
            let c3 = sample(m.tex1.w, at3, vec4<f32>(1.0));
            let h3 = sample(m.tex2.y, at3, NO_HEIGHT);
            let n3 = sample(m.tex2.x, at3, FLAT);
            tint_mask3 = contrast_brightness(h3.g, m.layer_a[2].x, m.layer_a[2].y);
            let albedo3 = layer_color(m, 2u, c3.rgb, tint_mask3, (flags & (MODEL_TINT_1 << 2u)) != 0u);
            if (flags & NEW_BLENDING) != 0u {
                w3 = blend_layer(carry2, bf3, h3.r, m.layer_a[2].w, m.layer_a[2].z, softness, m.layer_b[2].y, m.layer_b[2].z).x;
            } else {
                let zero = vec2<f32>(max(m.layer_a[0].w, m.layer_a[1].w), m.layer_a[2].w);
                let scale = vec2<f32>(max(m.layer_a[0].z, m.layer_a[1].z), m.layer_a[2].z);
                w3 = blend_weights(vec2<f32>(max(h1.r - m.layer_a[0].w, h2.r - m.layer_a[1].w), h3.r - m.layer_a[2].w), scale, zero, bf3, softness).y;
            }
            albedo = mix(albedo, albedo3, w3);
            normal = mix(normal, decode_normal(n3, false, m.layer_b[2].w), w3);
            roughness = mix(roughness, contrast_brightness(select(1.0, n3.b, m.tex2.x != NONE), m.layer_c[2].x, m.layer_c[2].y), w3);
            metalness = mix(metalness, h3.a * m.layer_c[2].z, w3);
            occlusion = mix(occlusion, c3.a, w3);
        }
    }
    // the vertex colour, by each layer's mode: masked by its tint mask, unmasked, or none
    let share2 = w2 * (1.0 - w3);
    let share1 = clamp(1.0 - share2 - w3, 0.0, 1.0);
    let mode = vec3<f32>(m.layer_c[0].w, m.layer_c[1].w, m.layer_c[2].w);
    var masked = tint_mask1 * share1 + tint_mask2 * share2 + tint_mask3 * w3;
    var unmasked = select(0.0, share1, mode.x != 0.0) + select(0.0, share2, mode.y != 0.0) + select(0.0, w3, mode.z != 0.0);
    let enabled = select(0.0, share1, mode.x != 2.0) + select(0.0, share2, mode.y != 2.0);
    var vertex_mask = clamp(unmasked + masked, 0.0, 1.0) * enabled + select(0.0, w3, mode.z != 2.0);
    if layers == 1u {
        // csgo_environment ignores the mode: masked by the height map's raw green
        vertex_mask = h1.g;
    }
    let vertex_color = mix(vec3<f32>(1.0), in.color.rgb, in.color.a);
    albedo *= mix(vec3<f32>(1.0), vertex_color, clamp(vertex_mask, 0.0, 1.0));
    s.albedo = albedo;
    s.alpha = c1.a;
    s.normal = normal;
    s.roughness = roughness;
    s.metalness = metalness;
    // the colour's alpha is occlusion, through the default levels (0, 0.5, 1): its square root
    s.occlusion = sqrt(max(occlusion, 0.0));
    return s;
}

fn blend_modulation(blend_factor: f32, mask: f32, softness: f32) -> f32 {
    return smoothstep(max(0.0, mask - softness), min(1.0, mask + softness), blend_factor);
}

fn two_layer(m: Material, in: Varyings, uv0: Uv) -> Surface {
    var s: Surface;
    let c1 = sample(m.tex0.x, uv0, vec4<f32>(1.0));
    let at2 = Uv(uv0.uv * m.uv[2].xy, uv0.dx * m.uv[2].xy, uv0.dy * m.uv[2].xy);
    let modulation = sample(m.tex2.w, at2, vec4<f32>(0.5, 0.5, 0.0, 0.5));
    let c2 = sample(m.tex2.z, at2, vec4<f32>(1.0));
    // the vertex paint (VertexPaintBlendParams' x), sharpened by the modulation texture as F_FANCY_BLENDING says
    var blend = in.paint.x;
    let fancy = m.tint2.w;
    if fancy > 2.5 {
        blend = blend_modulation(blend, modulation.a, m.tint.w);
    } else if fancy > 1.5 {
        blend = blend_modulation(blend, modulation.g, m.tint.w);
    } else if fancy > 0.5 {
        blend = blend_modulation(blend, modulation.g, modulation.r);
    }
    s.albedo = mix(c1.rgb * m.tint.rgb, c2.rgb * m.tint2.rgb, blend) * m.factor.rgb;
    s.alpha = c1.a * m.factor.a;
    let n = sample(m.tex0.y, uv0, FLAT);
    s.normal = decode_normal(n, (m.info.z & RGB_NORMAL_1) != 0u, 1.0);
    s.roughness = select(1.0, n.b, m.tex0.y != NONE && (m.info.z & RGB_NORMAL_1) == 0u);
    s.metalness = 0.0;
    s.occlusion = 1.0;
    return s;
}

fn generic(m: Material, in: Varyings, uv0: Uv) -> Surface {
    var s: Surface;
    let c = sample(m.tex0.x, uv0, vec4<f32>(1.0)) * m.factor;
    s.albedo = c.rgb * in.color.rgb;
    s.alpha = c.a;
    let n = sample(m.tex0.y, uv0, FLAT);
    s.normal = decode_normal(n, (m.info.z & RGB_NORMAL_1) != 0u, 1.0);
    s.roughness = select(1.0, n.b, m.tex0.y != NONE && (m.info.z & RGB_NORMAL_1) == 0u);
    s.metalness = 0.0;
    s.occlusion = sample(m.tex0.z, uv0, vec4<f32>(1.0)).r;
    return s;
}

// --- light ---

// a scene direction as the sky's cube map reads it: into Source's frame (z up), turned back by the sky's yaw
fn sky_direction(d: vec3<f32>) -> vec3<f32> {
    let source = vec3<f32>(d.z, d.x, d.y);
    let c = cos(frame.sky.w);
    let s = sin(frame.sky.w);
    return vec3<f32>(c * source.x + s * source.y, -s * source.x + c * source.y, source.z);
}

fn sky_at(d: vec3<f32>, lod: f32) -> vec3<f32> {
    return textureSampleLevel(sky_cube, clamp_sampler, sky_direction(d), lod).rgb * frame.sky.rgb;
}

// Cubic B-spline filtering of the baked shadows from four bilinear taps (Source 2 Viewer's SampleLightmapShadowsBicubic)
fn shadow_bicubic(t: u32, uv: vec2<f32>, size: f32) -> vec4<f32> {
    let texel = uv * size - 0.5;
    let base = floor(texel);
    let f = texel - base;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (-f3 + 3.0 * f2 - 3.0 * f + 1.0) / 6.0;
    let w1 = (3.0 * f3 - 6.0 * f2 + 4.0) / 6.0;
    let w2 = (-3.0 * f3 + 3.0 * f2 + 3.0 * f + 1.0) / 6.0;
    let w3 = f3 / 6.0;
    let weight0 = w0 + w1;
    let weight1 = w2 + w3;
    let uv0 = (base + w1 / weight0 - 1.5) / size;
    let uv1 = (base + w3 / weight1 + 0.5) / size;
    var s = textureSampleLevel(textures[t], clamp_sampler, vec2<f32>(uv0.x, uv0.y), 0.0) * weight0.x * weight0.y;
    s += textureSampleLevel(textures[t], clamp_sampler, vec2<f32>(uv1.x, uv0.y), 0.0) * weight1.x * weight0.y;
    s += textureSampleLevel(textures[t], clamp_sampler, vec2<f32>(uv0.x, uv1.y), 0.0) * weight0.x * weight1.y;
    s += textureSampleLevel(textures[t], clamp_sampler, vec2<f32>(uv1.x, uv1.y), 0.0) * weight1.x * weight1.y;
    return s;
}

// --- the light probes: an ambient cube per probe, blended across the volumes that reach a point (lighting.lpv.slang) ---

struct ProbeSample {
    irradiance: vec3<f32>,
    shadows: vec4<f32>,
    coverage: f32,
}

// the ambient cube along a normal in Source's axes: the faces it looks toward, weighed by the normal's squares
fn probe_ambient_cube(coords: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let offsets = select(vec3<f32>(0.0, 1.0, 2.0), vec3<f32>(3.0, 4.0, 5.0), normal < vec3<f32>(0.0)) / 6.0;
    let squared = normal * normal;
    var sum = textureSampleLevel(probe_irradiance, clamp_sampler, coords + vec3<f32>(0.0, 0.0, offsets.x), 0.0).rgb * squared.x;
    sum += textureSampleLevel(probe_irradiance, clamp_sampler, coords + vec3<f32>(0.0, 0.0, offsets.y), 0.0).rgb * squared.y;
    sum += textureSampleLevel(probe_irradiance, clamp_sampler, coords + vec3<f32>(0.0, 0.0, offsets.z), 0.0).rgb * squared.z;
    return sum;
}

fn min3(v: vec3<f32>) -> f32 {
    return min(v.x, min(v.y, v.z));
}

// every volume that reaches `world`, by priority, each taking what the ones before left uncovered
fn sample_probes(world: vec3<f32>, normal: vec3<f32>) -> ProbeSample {
    var p = ProbeSample(vec3<f32>(0.0), vec4<f32>(0.0), 0.0);
    let n = vec3<f32>(normal.z, normal.x, normal.y);
    let count = u32(frame.probe_info.x);
    for (var i = 0u; i < count; i++) {
        if p.coverage > 0.99 {
            break;
        }
        let v = probe_volumes[i];
        let local = (v.to_local * vec4<f32>(world, 1.0)).xyz;
        let from_min = clamp((local + v.bounds_extend.xyz) * v.inverse_edge.xyz, vec3<f32>(0.0), vec3<f32>(1.0));
        let from_max = clamp((1.0 - local + v.bounds_extend.xyz) * v.inverse_edge.xyz, vec3<f32>(0.0), vec3<f32>(1.0));
        let weight = smoothstep(0.0, 1.0, min(min3(from_min), min3(from_max))) * (1.0 - p.coverage);
        if weight <= 0.0 {
            continue;
        }
        // half a probe inside the box (z in sixths: one slab of the six), then into the atlas
        var c = local * vec3<f32>(1.0, 1.0, 1.0 / 6.0);
        c = clamp(c, v.border_min.xyz, v.border_max.xyz) * vec3<f32>(1.0, 1.0, 6.0);
        c = (c * v.atlas_scale.xyz + v.atlas_offset.xyz) * vec3<f32>(1.0, 1.0, 1.0 / 6.0);
        p.irradiance += probe_ambient_cube(c, n) * weight;
        let s = clamp(local, vec3<f32>(0.0), vec3<f32>(1.0)) * v.atlas_scale.xyz + v.atlas_offset.xyz;
        p.shadows += textureSampleLevel(probe_shadows, clamp_sampler, s, 0.0) * weight;
        p.coverage += weight;
    }
    return p;
}

const DIRECTION_MAX_XY: f32 = 0.99619;

// the directional lightmap's shading of a bumped surface (ComputeLightmapShading, lightmap version 2): the dominant
// direction kept 5° off the surface; an unbumped surface gets the irradiance back as it is
fn lightmap_shading(irradiance: vec3<f32>, direction: vec3<f32>, normal_ts: vec3<f32>) -> vec3<f32> {
    var xy = direction.xy * 2.0 - 1.0;
    xy *= DIRECTION_MAX_XY / max(DIRECTION_MAX_XY, length(xy));
    let light = vec3<f32>(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
    let non_directional = irradiance * clamp(direction.z, 0.0, 1.0);
    let n_dot_l = max(dot(light, normal_ts), 0.0);
    return n_dot_l * (irradiance - non_directional) / light.z + non_directional;
}

fn d_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a = n_dot_h * alpha;
    let k = min(alpha / (1.0 - n_dot_h * n_dot_h + a * a), 453.5);
    return k * k;
}

fn vis_schlick(n_dot_l: f32, n_dot_v: f32, roughness: f32) -> f32 {
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    return 1.0 / (4.0 * max((n_dot_l * (1.0 - k) + k) * (n_dot_v * (1.0 - k) + k), 0.0001));
}

// Karis' analytic fit of the split-sum environment BRDF
fn env_brdf(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

fn light(s: Surface, in: Varyings, lightmap: Uv, front: bool, double_sided: bool) -> vec3<f32> {
    var ng = normalize(in.normal);
    var tangent = normalize(in.tangent.xyz);
    if double_sided && !front {
        ng = -ng;
    }
    let bitangent = cross(ng, tangent) * in.tangent.w;
    let n = normalize(tangent * s.normal.x + bitangent * s.normal.y + ng * s.normal.z);
    let v = normalize(frame.eye.xyz - in.world);
    let n_dot_v = max(dot(n, v), 0.0);

    var indirect = vec3<f32>(0.0);
    var visibility = 1.0;
    let lm = frame.lightmaps;
    if in.lit != 0u && lm.x != NONE {
        let irradiance = textureSampleGrad(textures[lm.x], clamp_sampler, lightmap.uv, lightmap.dx, lightmap.dy).rgb;
        if lm.y != NONE {
            let direction = textureSampleGrad(textures[lm.y], clamp_sampler, lightmap.uv, lightmap.dx, lightmap.dy).rgb;
            indirect = lightmap_shading(irradiance, direction, s.normal);
            if debug_mode == 6u {
                return direction;
            }
            if debug_mode == 7u {
                return s.normal * 0.5 + 0.5;
            }
        } else {
            indirect = irradiance;
        }
        if debug_mode == 5u {
            return irradiance;
        }
        if lm.z != NONE {
            var shadows: vec4<f32>;
            if lm.w != 0u {
                shadows = shadow_bicubic(lm.z, lightmap.uv, frame.sky_info.w);
            } else {
                shadows = textureSampleLevel(textures[lm.z], clamp_sampler, lightmap.uv, 0.0);
            }
            visibility = clamp(1.0 - dot(shadows, frame.shadow_mask), 0.0, 1.0);
        }
    } else {
        // no lightmap: the light probes, and the sky's light around the normal for what no volume covers
        var coverage = 0.0;
        if frame.probe_info.x > 0.0 {
            let probe = sample_probes(in.world, n);
            indirect = probe.irradiance;
            coverage = probe.coverage;
            if coverage > 0.0 {
                visibility = clamp(1.0 - dot(probe.shadows, frame.shadow_mask) / coverage, 0.0, 1.0);
            }
        }
        if coverage < 0.99 {
            let fill = select(vec3<f32>(0.3), sky_at(n, frame.sky_info.z) * 0.5, frame.sky_info.x > 0.5);
            indirect += fill * (1.0 - coverage);
        }
    }

    let diffuse_color = s.albedo * (1.0 - s.metalness);
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metalness);
    let roughness = clamp(s.roughness, 0.03, 1.0);
    var direct = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);
    if frame.sun_dir.w > 0.5 && visibility > 0.0001 {
        let l = frame.sun_dir.xyz;
        let n_dot_l = max(dot(n, l), 0.0);
        let sun = frame.sun_color.rgb * visibility;
        direct = sun * n_dot_l;
        let h = normalize(v + l);
        let n_dot_h = max(dot(n, h), 0.0);
        let v_dot_h = max(dot(l, h), 0.0);
        let f = mix(f0, vec3<f32>(1.0), pow(1.0 - v_dot_h, 5.0));
        specular = d_ggx(n_dot_h, max(roughness * roughness, 0.0001)) * vis_schlick(n_dot_l, n_dot_v, roughness) * n_dot_l * f * sun;
    }
    // the sky's reflection, no brighter than the light that reaches here (CS2's normalisation, capped at 1)
    if frame.sky_info.x > 0.5 {
        let r = reflect(-v, n);
        let environment = sky_at(r, roughness * frame.sky_info.y);
        let sky_light = dot(sky_at(n, frame.sky_info.z), LUMA) + 0.0001;
        let scale = min(dot(indirect, LUMA) / sky_light, 1.0);
        specular += environment * scale * env_brdf(f0, roughness, n_dot_v);
    }
    let occlusion = s.occlusion;
    if debug_mode == 1u && in.lit == 0u {
        return vec3<f32>(1.0, 0.0, 1.0);
    }
    if debug_mode == 2u {
        return indirect;
    }
    if debug_mode == 3u {
        return vec3<f32>(visibility);
    }
    if debug_mode == 4u {
        return s.albedo;
    }
    return diffuse_color * (direct + indirect) * occlusion + specular * occlusion;
}

// --- fog (CS2's gradient and cubemap fog, in this pass's space) ---

// how much fog, and toward what
fn fog(world: vec3<f32>) -> vec4<f32> {
    let rel = world - frame.eye.xyz;
    let dist = length(rel);
    var color = vec3<f32>(0.0);
    var keep = 1.0;
    let g = frame.fog_gradient;
    if frame.fog_gradient_color.a > 0.0 {
        let c = pow(clamp(vec2<f32>(dist, world.y) * g.zw + g.xy, vec2<f32>(0.0001), vec2<f32>(1.0)), frame.fog_gradient_exp.xy);
        let f = c.x * c.y * frame.fog_gradient_color.a;
        color = mix(color, frame.fog_gradient_color.rgb, f);
        keep *= 1.0 - f;
    }
    let cube = frame.fog_cube;
    let height = frame.fog_cube_height;
    if frame.fog_cube_color.y > 0.0 && frame.sky_info.x > 0.5 {
        let x = pow(max(dist * cube.y + cube.x, 0.0001), cube.w);
        let y = pow(max(world.y * height.y + height.x, 0.0001), height.z);
        let blend = clamp(x, 0.0, 1.0) * clamp(y, 0.0, 1.0);
        let lod = clamp(1.0 - blend * cube.z, 0.0, 1.0) * height.w;
        let f = blend * frame.fog_cube_color.y;
        let sky = textureSampleLevel(sky_cube, clamp_sampler, sky_direction(rel / max(dist, 0.0001)), lod).rgb * frame.fog_cube_color.x;
        color = mix(color, sky, f);
        keep *= 1.0 - f;
    }
    if keep >= 1.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(color / (1.0 - keep), 1.0 - keep);
}

fn fogged(c: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let f = fog(world);
    return mix(c, f.rgb, f.a);
}

// --- the fragment entry points ---

struct Shaded {
    color: vec3<f32>,
    alpha: f32,
}

fn uvs(in: Varyings) -> array<Uv, 3> {
    return array<Uv, 3>(
        Uv(in.uv.xy, dpdx(in.uv.xy), dpdy(in.uv.xy)),
        Uv(in.uv.zw, dpdx(in.uv.zw), dpdy(in.uv.zw)),
        Uv(in.lightmap_uv, dpdx(in.lightmap_uv), dpdy(in.lightmap_uv)),
    );
}

fn shade(in: Varyings, front: bool) -> Shaded {
    let all = uvs(in);
    let m = materials[in.material];
    let kind = m.info.x;
    var out: Shaded;
    if kind == 4u {
        // unlit
        let c = sample(m.tex0.x, all[0], vec4<f32>(1.0)) * m.factor;
        out.color = c.rgb;
        out.alpha = c.a;
        if (m.info.z & ADDITIVE) != 0u {
            out.color *= 1.0 - fog(in.world).a;
        } else {
            out.color = fogged(out.color, in.world);
        }
        return out;
    }
    if kind == 5u {
        out.color = fogged(vec3<f32>(0.0), in.world);
        out.alpha = 1.0;
        return out;
    }
    var s: Surface;
    if kind == 1u {
        s = environment(m, in, all[0], all[1]);
    } else if kind == 2u {
        s = two_layer(m, in, all[0]);
    } else {
        s = generic(m, in, all[0]);
    }
    out.color = fogged(light(s, in, all[2], front, (m.info.z & DOUBLE_SIDED) != 0u), in.world);
    out.alpha = s.alpha;
    return out;
}

@fragment
fn opaque(in: Varyings, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in, front).color, 1.0);
}

@fragment
fn mask(in: Varyings, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let shaded = shade(in, front);
    if shaded.alpha < bitcast<f32>(materials[in.material].info.w) {
        discard;
    }
    return vec4<f32>(shaded.color, 1.0);
}

@fragment
fn blend(in: Varyings, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let shaded = shade(in, front);
    return vec4<f32>(shaded.color, clamp(shaded.alpha, 0.0, 1.0));
}

// csgo_effects: unlit, faded edge-on, up close and where it meets what is behind it
@fragment
fn effects(in: Varyings, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let all = uvs(in);
    let m = materials[in.material];
    let c = sample(m.tex0.x, all[0], vec4<f32>(1.0));
    let tint = in.color * m.factor;
    var opacity = c.a * tint.a * m.effects.w;
    let time = frame.eye.w;
    for (var k = 0u; k < 3u; k++) {
        if (m.info.z & (MASK_1 << k)) != 0u {
            let pan = m.masks[k];
            let offset = vec2<f32>(0.37, 0.61) * f32(k) + pan.zw * time;
            opacity *= textureSampleGrad(noise, material_sampler, all[0].uv * pan.xy + offset, all[0].dx * pan.xy, all[0].dy * pan.xy).r;
        }
    }
    let ray = in.world - frame.eye.xyz;
    let along = length(ray);
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }
    var fresnel = clamp(dot(-ray / max(along, 0.0001), n), 0.0001, 1.0);
    fresnel = clamp(pow(fresnel, m.fresnel.x) * m.fresnel.y, 0.0, 1.0);
    fresnel = mix(m.fresnel.z, m.fresnel.w, fresnel);
    var fade = clamp(along / m.fade.x, 0.0, 1.0);
    fade = mix(m.fade.z, m.fade.w, fade);
    fade = pow(max(fade, 0.000001), m.fade.y);
    var feather = 1.0;
    if m.effects.x > 0.0 {
        let size = vec2<i32>(textureDimensions(scene_depth));
        let pixel = clamp(vec2<i32>(in.clip.xy), vec2<i32>(0), size - 1);
        let depth = textureLoad(scene_depth, pixel, 0);
        if depth > 0.0 {
            // reversed depth: view distance = near × far / (near + depth × (far − near))
            let near = frame.screen.z;
            let far = frame.screen.w;
            let scene_distance = near * far / (near + depth * (far - near));
            let here = near * far / (near + in.clip.z * (far - near));
            let behind = (scene_distance - here) * along / max(here, 0.0001);
            feather = pow(clamp(behind / m.effects.x, 0.0001, 1.0), m.effects.y);
        }
    }
    opacity *= fresnel * fade * feather;
    var rgb = c.rgb * tint.rgb * m.effects.z;
    let f = fog(in.world);
    if (m.info.z & ADDITIVE) != 0u {
        rgb *= 1.0 - f.a;
    } else {
        rgb = mix(rgb, f.rgb, f.a);
    }
    return vec4<f32>(rgb, clamp(opacity, 0.0, 1.0));
}

// --- the sky: a full-screen triangle at the far plane, each pixel the sky toward its ray ---

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn sky_vertex(@builtin(vertex_index) v: u32) -> SkyOut {
    let xy = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u)) * 2.0 - 1.0;
    var out: SkyOut;
    out.clip = vec4<f32>(xy, 0.0, 1.0);
    out.ndc = xy;
    return out;
}

@fragment
fn sky(in: SkyOut) -> @location(0) vec4<f32> {
    let far = frame.inverse_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let near = frame.inverse_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let d = normalize(far.xyz / far.w - near.xyz / near.w);
    if frame.sky_info.x < 0.5 {
        return vec4<f32>(0.66, 0.76, 0.86, 1.0);
    }
    return vec4<f32>(sky_at(d, 0.0), 1.0);
}
