//! What the packer has Source 2 Viewer's CLI decompile from the game's files, and the records it reads from them: the
//! map's entity lump (and its 3D skybox prefab's), the materials of its skies and the post-processing resource with
//! its colour lookup table. They give the records show-me-the-nades' `export_map.py` wrote beside the export
//! (`<map>.post.json`, `<map>.lut.bin`, `<map>.sky.json`, `<map>.skybox.json`), by the same rules, ported line for line
//! from its `post`, `sky`, `skybox`, `visible_sky` and `gradient_fog`, so the packs come out the same:
//!
//! - entities are the lump's blocks split at blank lines; a key's value is its line's rest (`key   value`);
//!   numbers are every `-?digits(.digits)?` in it; an entity starts on unless `startdisabled` is `true` or
//!   `enabled` is `false`;
//! - the sky the game draws is the 3D skybox prefab's first env_sky that starts on and names a `.vmat`, else the
//!   map's; the 3D skybox is the prefab its first `skybox_reference` names, placed by the prefab's `sky_camera`;
//! - the post-processing is the master `post_processing_volume` (else the first) with a resource: its exposure range,
//!   and from its `.vpost` the tone curve, bloom and lookup table; the fog is the first env_cubemap_fog that starts on,
//!   its sky's exposure the sky material's biases plus log2 of the sky's brightness; the sun is the first
//!   light_environment that starts on.
//!
//! One difference: export_map.py wrote `<map>.sky.json` only once the sky's texture decompiled to EXR (for the web
//! view's HDR file); here the texture only has to be in a `pak01` archive the depot holds, which is what the pack reads.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, ensure};
use nades_source2::vpk::Vpk;
use serde_json::{Value, json};

/// Runs the CLI on one entry of a VPK at a time, decompiling into a scratch folder that goes when this does.
pub struct Decompiler<'a> {
    cli: &'a Path,
    dir: PathBuf,
    runs: u32,
}

impl<'a> Decompiler<'a> {
    pub fn new(cli: &'a Path, scratch: &Path, label: &str) -> Result<Self> {
        let dir = scratch.join(format!(".nades-pack-{label}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { cli, dir, runs: 0 })
    }

    /// `entry` of `vpk` decompiled (`-d`): the folder it went to (the entry's path under it); the CLI's own failures
    /// leave the file out.
    pub fn decompile(&mut self, vpk: &Path, entry: &str) -> Result<PathBuf> {
        self.runs += 1;
        let out = self.dir.join(format!("{}", self.runs));
        let status = Command::new(self.cli)
            .arg("-i")
            .arg(vpk)
            .args(["-f", entry, "-d", "-o"])
            .arg(&out)
            // the CLI writes exceptions.txt where it runs
            .current_dir(&self.dir)
            .output()
            .with_context(|| format!("running {}", self.cli.display()))?
            .status;
        ensure!(status.success(), "{} exited {status} decompiling {entry}", self.cli.display());
        Ok(out)
    }

    /// A decompiled text file, or None when the CLI wrote none.
    pub fn text(&mut self, vpk: &Path, entry: &str) -> Result<Option<String>> {
        let out = self.decompile(vpk, entry)?;
        let path = out.join(entry.strip_suffix("_c").unwrap_or(entry));
        Ok(std::fs::read(&path).ok().map(|b| String::from_utf8_lossy(&b).into_owned()))
    }
}

impl Drop for Decompiler<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The map's entities' blocks, as export_map.py split them.
pub fn blocks(lump: &str) -> Vec<&str> {
    lump.split("\n\n").collect()
}

/// One entity key's raw value: `key   value` on its own line, the value trimmed; the first such line.
pub fn keyvalue<'b>(block: &'b str, key: &str) -> Option<&'b str> {
    // Python's (?m)^key\s+(.+?)\s*$: whitespace after the key may run over line ends
    let mut at = 0;
    while let Some(found) = block[at..].find(key) {
        let start = at + found;
        at = start + key.len();
        if start != 0 && block.as_bytes()[start - 1] != b'\n' {
            continue;
        }
        let rest = &block[start + key.len()..];
        let value_at = rest.len() - rest.trim_start().len();
        if value_at == 0 {
            continue;
        }
        let rest = &rest[value_at..];
        let line = rest.split('\n').next().unwrap_or("").trim_end();
        if !line.is_empty() {
            return Some(line);
        }
    }
    None
}

/// Every `-?[0-9]+(\.[0-9]+)?` in `text`.
pub fn numbers(text: &str) -> Vec<f64> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        if b[i] == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if !b[i].is_ascii_digit() {
            i = start + 1;
            continue;
        }
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
        out.push(text[start..i].parse().expect("digits"));
    }
    out
}

/// One numeric key, quoted or not.
pub fn number(block: &str, key: &str, default: f64) -> f64 {
    numbers(keyvalue(block, key).unwrap_or("")).first().copied().unwrap_or(default)
}

/// `float(keyvalue(block, key) or default)`.
fn float(block: &str, key: &str, default: f64) -> f64 {
    keyvalue(block, key).and_then(|v| v.trim_matches('"').parse().ok()).unwrap_or(default)
}

/// The entities of one class.
pub fn classes<'b>(blocks: &[&'b str], name: &str) -> Vec<&'b str> {
    blocks.iter().copied().filter(|b| keyvalue_lines(b, "classname").any(|v| v == format!("\"{name}\""))).collect()
}

/// Every value a key has on a line of its own (for `classname`, which the regex matched on any line).
fn keyvalue_lines<'b>(block: &'b str, key: &'b str) -> impl Iterator<Item = &'b str> + 'b {
    block.lines().filter_map(move |l| {
        let rest = l.strip_prefix(key)?;
        let value = rest.trim_start();
        (value.len() < rest.len()).then(|| value.trim_end())
    })
}

/// Whether an entity starts on.
pub fn enabled(block: &str) -> bool {
    keyvalue(block, "startdisabled") != Some("true") && keyvalue(block, "enabled") != Some("false")
}

/// The first `prefix"(…suffix)"` after `key` and whitespace: `skyname resource_name:"x.vmat"` and the like.
fn quoted_after(text: &str, key: &str, prefix: &str, suffix: &str) -> Option<String> {
    let mut at = 0;
    while let Some(found) = text[at..].find(key) {
        let start = at + found + key.len();
        at = start;
        let rest = &text[start..];
        let trimmed = rest.trim_start();
        if trimmed.len() == rest.len() && !key.ends_with(':') {
            continue;
        }
        let trimmed = trimmed.strip_prefix(prefix).unwrap_or(trimmed);
        let Some(value) = trimmed.strip_prefix('"') else { continue };
        let Some(end) = value.find('"') else { continue };
        let value = &value[..end];
        if value.ends_with(suffix) && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// An env_sky's material: `skyname (resource_name:)?"….vmat"`.
fn skyname(block: &str) -> Option<String> {
    quoted_after(block, "skyname", "resource_name:", ".vmat")
}

/// The first `resource_name:"….<ext>"` in a value.
fn resource(value: &str, ext: &str) -> Option<String> {
    let mut rest = value;
    while let Some(at) = rest.find("resource_name:\"") {
        rest = &rest[at + "resource_name:\"".len()..];
        let end = rest.find('"')?;
        if rest[..end].ends_with(ext) {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// The first env_gradient_fog that starts on.
pub fn gradient_fog(blocks: &[&str]) -> Value {
    let Some(fog) = classes(blocks, "env_gradient_fog").into_iter().find(|b| enabled(b)) else { return Value::Null };
    let color: Vec<f64> = numbers(keyvalue(fog, "fogcolor").unwrap_or("255 255 255")).into_iter().take(3).collect();
    json!({
        "start": number(fog, "fogstart", 0.0),
        "end": number(fog, "fogend", 4000.0),
        "falloff": number(fog, "fogfalloffexponent", 1.0),
        "height": keyvalue(fog, "heightfog") != Some("false"),
        "heightStart": number(fog, "fogstartheight", 0.0),
        "heightEnd": number(fog, "fogendheight", 0.0),
        "verticalExponent": number(fog, "fogverticalexponent", 1.0),
        "color": color,
        "strength": number(fog, "fogstrength", 1.0),
        "maxOpacity": number(fog, "fogmaxopacity", 1.0),
    })
}

/// What the map's files give the pack, in export_map.py's records.
#[derive(Debug, Default)]
pub struct Lump {
    /// The map's entity lump.
    pub entities: String,
    /// `<map>.post.json`.
    pub post: Value,
    /// `<map>.lut.bin`: dim³ RGB bytes.
    pub lut: Option<Vec<u8>>,
    /// `<map>.sky.json`, when the game draws a sky whose texture the depot holds.
    pub sky: Value,
    /// The drawn sky's material, decompiled.
    pub sky_vmat: Option<String>,
    /// `<map>.skybox.json`, when the map has a 3D skybox whose prefab the depot holds.
    pub skybox: Value,
    /// CLI runs.
    pub runs: u32,
}

/// The game's files a map's records come from.
pub struct Game<'g> {
    /// `game/csgo/maps`.
    pub maps: PathBuf,
    /// `pak01_dir.vpk`, opened, when the game folder has it.
    pub pak: Option<&'g Vpk>,
    pub pak_path: PathBuf,
}

impl Game<'_> {
    /// Whether `pak01` holds the entry's bytes.
    fn held(&self, entry: &str) -> bool {
        self.pak.is_some_and(|pak| pak.entry(entry).is_some_and(|e| e.length == 0 || e.archive == nades_source2::vpk::IN_DIRECTORY || pak.archive_path(e.archive).is_file()))
    }
}

/// The map's (or a prefab's) entity lump, or None without one.
fn entities(d: &mut Decompiler, game: &Game, name: &str) -> Result<Option<String>> {
    let vpk = game.maps.join(format!("{name}.vpk"));
    if !vpk.is_file() {
        return Ok(None);
    }
    d.text(&vpk, &format!("maps/{name}/entities/default_ents.vents_c"))
}

/// A material from `pak01`, decompiled.
fn material(d: &mut Decompiler, game: &Game, material: &str) -> Result<Option<String>> {
    let compiled = format!("{material}_c");
    if !game.held(&compiled) {
        return Ok(None);
    }
    d.text(&game.pak_path, &compiled)
}

/// A sky material's `g_flBrightnessExposureBias` and `g_flRenderOnlyExposureBias`, in stops (0 when unset).
fn biases(text: Option<&str>) -> (f64, f64) {
    let value = |name: &str| -> f64 {
        let Some(text) = text else { return 0.0 };
        let key = format!("\"{name}\"");
        let Some(at) = text.find(&key) else { return 0.0 };
        let rest = text[at + key.len()..].trim_start();
        rest.strip_prefix('"').and_then(|r| r.split('"').next()).and_then(|v| numbers(v).first().copied()).unwrap_or(0.0)
    };
    (value("g_flBrightnessExposureBias"), value("g_flRenderOnlyExposureBias"))
}

/// A vpost layer's float field: `key = number`.
fn field(part: Option<&str>, key: &str) -> Option<f64> {
    let part = part?;
    let mut at = 0;
    while let Some(found) = part[at..].find(key) {
        let start = at + found + key.len();
        at = start;
        let rest = part[start..].trim_start();
        if let Some(value) = rest.strip_prefix('=') {
            let value = value.trim_start();
            let end = value.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(value.len());
            if let Ok(v) = value[..end].parse() {
                return Some(v);
            }
        }
    }
    None
}

/// A vpost layer's `m_blendMode = "BLOOM_…"`.
fn blend_mode(part: &str) -> Option<String> {
    let mut at = 0;
    while let Some(found) = part[at..].find("m_blendMode") {
        let start = at + found + "m_blendMode".len();
        at = start;
        let Some(rest) = part[start..].trim_start().strip_prefix('=') else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('"') else { continue };
        let end = rest.find(|c: char| !(c.is_ascii_uppercase() || c == '_')).unwrap_or(rest.len());
        if end > 0 && rest[end..].starts_with('"') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// The sky texture a decompiled sky material names (`g_tSkyTexture`), else the one compiled texture `pak01` holds
/// under the material's name.
pub fn sky_texture(vmat: Option<&str>, material: &str, pak: &Vpk) -> Option<String> {
    if let Some(text) = vmat {
        let mut rest = text;
        while let Some(at) = rest.find("\"g_tSkyTexture\"") {
            rest = &rest[at + "\"g_tSkyTexture\"".len()..];
            if let Some(path) = rest.split('"').nth(1).filter(|p| p.ends_with(".vtex")) {
                return Some(path.to_string());
            }
        }
    }
    let stem = material.strip_suffix(".vmat").unwrap_or(material);
    let prefix = format!("{stem}_");
    let mut found: Vec<&str> = pak.entries().iter().map(|e| e.path.as_str()).filter(|p| p.starts_with(&prefix) && p.ends_with(".vtex_c")).collect();
    found.sort_unstable();
    found.first().map(|p| p.trim_end_matches("_c").to_string())
}

/// Everything the map's records need, decompiled by the CLI.
pub fn read(d: &mut Decompiler, game: &Game, map: &str) -> Result<Lump> {
    let mut lump = Lump::default();
    let Some(text) = entities(d, game, map)? else {
        lump.runs = d.runs;
        return Ok(lump);
    };
    let map_blocks = blocks(&text);
    // the 3D skybox: the prefab the first skybox_reference names
    let reference = classes(&map_blocks, "skybox_reference").into_iter().next();
    let target = reference.and_then(|r| quoted_after(r, "targetmapname", "", ".vmap")).and_then(|t| t.strip_prefix("maps/").and_then(|t| t.strip_suffix(".vmap")).map(str::to_string));
    let prefab = match (reference, target) {
        (Some(reference), Some(name)) => entities(d, game, &name)?.map(|t| (name, reference, t)),
        _ => None,
    };
    let prefab_blocks: Vec<&str> = prefab.as_ref().map(|(_, _, t)| blocks(t)).unwrap_or_default();
    if let Some((name, reference, _)) = &prefab {
        let camera = classes(&prefab_blocks, "sky_camera").into_iter().next();
        let three = |text: Option<&str>| -> Vec<f64> { numbers(text.unwrap_or("0 0 0")).into_iter().take(3).collect() };
        lump.skybox = json!({
            "prefab": name,
            "scale": camera.map_or(1.0, |c| number(c, "scale", 1.0)),
            "origin": camera.map_or(vec![0.0; 3], |c| three(keyvalue(c, "origin"))),
            "reference": three(keyvalue(reference, "origin")),
            "gradientFog": gradient_fog(&prefab_blocks),
        });
    }
    // the sky the game draws
    let drawn = [&prefab_blocks, &map_blocks].into_iter().flat_map(|b| classes(b, "env_sky")).find(|b| enabled(b) && skyname(b).is_some());
    let mut materials: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    let mut vmat = |d: &mut Decompiler, name: &str| -> Result<Option<String>> {
        if let Some(text) = materials.get(name) {
            return Ok(text.clone());
        }
        let text = material(d, game, name)?;
        materials.insert(name.to_string(), text.clone());
        Ok(text)
    };
    if let Some(block) = drawn
        && let Some(name) = skyname(block)
    {
        let text = vmat(d, &name)?;
        let texture = game.pak.and_then(|pak| sky_texture(text.as_deref(), &name, pak));
        if texture.is_some_and(|t| game.held(&format!("{t}_c"))) {
            let angles = numbers(keyvalue(block, "angles").unwrap_or("0 0 0"));
            lump.sky = json!({ "yaw": angles.get(1).copied().unwrap_or(0.0), "brightness": number(block, "brightnessscale", 1.0), "material": name });
        }
        lump.sky_vmat = text;
    }
    // the post-processing volume and its resource
    let of = |name: &str| classes(&map_blocks, name);
    let mut post = json!({ "tonemap": null, "bloom": null, "lut": null, "exposure": null, "fog": null, "gradientFog": null, "sun": null, "sky": null });
    let volumes: Vec<&str> = of("post_processing_volume").into_iter().filter(|b| keyvalue(b, "postprocessing").is_some()).collect();
    let volume = volumes.iter().copied().find(|b| keyvalue(b, "master") == Some("true")).or(volumes.first().copied());
    if let Some(volume) = volume {
        let flag = |k: &str, d: &str| keyvalue(volume, k).unwrap_or(d) == "true";
        post["exposure"] = json!({
            "enabled": flag("enableexposure", "false"),
            "min": float(volume, "minexposure", 1.0),
            "max": float(volume, "maxexposure", 1.0),
            "compensation": float(volume, "exposurecompensation", 0.0),
        });
        let path = resource(keyvalue(volume, "postprocessing").unwrap_or(""), ".vpost").map(|r| format!("{r}_c"));
        if let Some(path) = path.filter(|p| game.held(p)) {
            let out = d.decompile(&game.pak_path, &path)?;
            if let Ok(text) = std::fs::read(out.join(path.strip_suffix("_c").unwrap_or(&path))) {
                let text = String::from_utf8_lossy(&text);
                let layers: Vec<&str> = text.split("_class = \"").collect();
                let layer = |name: &str| layers.iter().copied().find(|p| p.starts_with(name));
                if let Some(tone) = layer("CToneMappingLayer") {
                    let keys = ["ExposureBias", "ShoulderStrength", "LinearStrength", "LinearAngle", "ToeStrength", "ToeNum", "ToeDenom", "WhitePoint"];
                    let mut map = serde_json::Map::new();
                    for k in keys {
                        let name = format!("{}{}", k[..1].to_lowercase(), &k[1..]);
                        map.insert(name, field(Some(tone), &format!("m_fl{k}")).map_or(Value::Null, Value::from));
                    }
                    map.insert("opacity".into(), Value::from(field(Some(tone), "m_nOpacityPercent").unwrap_or(100.0) / 100.0));
                    post["tonemap"] = Value::Object(map);
                }
                if let Some(bloom) = layer("CBloomLayer") {
                    let mode = blend_mode(bloom);
                    post["bloom"] = json!({
                        "mode": mode,
                        "strength": field(Some(bloom), "m_flBloomStrength").unwrap_or(0.0),
                        "screen": field(Some(bloom), "m_flScreenBloomStrength").unwrap_or(0.0),
                        "blur": field(Some(bloom), "m_flBlurBloomStrength").unwrap_or(0.0),
                        "threshold": field(Some(bloom), "m_flBloomThreshold").unwrap_or(1.0),
                        "thresholdWidth": field(Some(bloom), "m_flBloomThresholdWidth").unwrap_or(1.0),
                        "opacity": field(Some(bloom), "m_nOpacityPercent").unwrap_or(100.0) / 100.0,
                    });
                }
                let lut = layer("CColorLookupColorCorrectionLayer");
                let dim = field(lut, "m_nDim").unwrap_or(0.0) as usize;
                if lut.is_some() && dim > 0
                    && let Some(raw) = first_raw(&out)
                    && raw.len() == dim.pow(3) * 3
                {
                    post["lut"] = json!({ "dim": dim, "weight": field(lut, "m_nOpacityPercent").unwrap_or(100.0) / 100.0 });
                    lump.lut = Some(raw);
                }
            }
        }
    }
    // the fog, its sky's exposure
    if let Some(fog) = of("env_cubemap_fog").into_iter().find(|b| enabled(b)) {
        let n = |k: &str, d: f64| number(fog, k, d);
        let mut value = json!({
            "start": n("cubemapfogstartdistance", 0.0),
            "end": n("cubemapfogenddistance", 8000.0),
            "falloff": n("cubemapfogfalloffexponent", 1.0),
            "height": keyvalue(fog, "cubemapheightfog") != Some("false"),
            "heightStart": n("cubemapfogheightstart", 0.0),
            "heightEnd": n("cubemapfogheightend", 4000.0),
            "heightExponent": n("cubemapfogheightexponent", 1.0),
            "maxOpacity": n("cubemapfogmaxopacity", 1.0),
            "lodBias": n("cubemapfoglodbiase", 0.0),
            "sky": null,
        });
        let (mut material_name, mut brightness) = (None, 1.0);
        match n("cubemapfogsource", 0.0) as i64 {
            1 => {
                let name = keyvalue(fog, "cubemapfogskyentity");
                let target = of("env_sky").into_iter().find(|b| name.is_some() && keyvalue(b, "targetname") == name);
                if let Some(target) = target
                    && let Some(found) = skyname(target)
                {
                    material_name = Some(found);
                    brightness = number(target, "brightnessscale", 1.0);
                }
            }
            2 => material_name = resource(keyvalue(fog, "cubemapfogskymaterial").unwrap_or(""), ".vmat"),
            _ => {}
        }
        if let Some(name) = material_name {
            let (a, b) = biases(vmat(d, &name)?.as_deref());
            value["sky"] = json!({ "material": name, "brightness": brightness, "exposureBias": a + b + brightness.max(1e-6).log2() });
        }
        post["fog"] = value;
    }
    post["gradientFog"] = gradient_fog(&map_blocks);
    if let Some(block) = drawn
        && let Some(name) = skyname(block)
    {
        let (bias, render_only) = biases(vmat(d, &name)?.as_deref());
        let tint: Vec<f64> = numbers(keyvalue(block, "tint_color").unwrap_or("255 255 255")).into_iter().take(3).collect();
        post["sky"] = json!({ "material": name, "exposureBias": bias, "renderOnlyExposureBias": render_only, "tint": tint });
    }
    if let Some(sun) = of("light_environment").into_iter().find(|b| enabled(b)) {
        let three = |k: &str| -> Vec<f64> { numbers(keyvalue(sun, k).unwrap_or("255 255 255")).into_iter().take(3).collect() };
        post["sun"] = json!({ "color": three("color"), "brightness": float(sun, "brightness", 1.0), "skyColor": three("skycolor"), "skyIntensity": float(sun, "skyintensity", 1.0) });
    }
    lump.post = post;
    lump.entities = text;
    lump.runs = d.runs;
    Ok(lump)
}

/// The first `.raw` file under `dir` (the lookup table the CLI writes beside the vpost).
fn first_raw(dir: &Path) -> Option<Vec<u8>> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "raw") {
                return std::fs::read(path).ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_numbers_read_as_export_map_read_them() {
        let block = "====4====\nclassname                      \"env_sky\"\nskyname                        resource_name:\"materials/sky/x.vmat\"\nangles                         [ 0.0, 116.0, 0.0 ]\nbrightnessscale                4.0\nstartdisabled                  false\ntint_color                     [ 255, 250, 240, 255 ]\n";
        assert_eq!(keyvalue(block, "angles"), Some("[ 0.0, 116.0, 0.0 ]"));
        assert_eq!(keyvalue(block, "brightness"), None, "a key is a whole word");
        assert_eq!(numbers("[ -1.5, 2, 1e-05 ]"), vec![-1.5, 2.0, 1.0, -5.0], "exponents read as export_map.py read them");
        assert_eq!(number(block, "brightnessscale", 1.0), 4.0);
        assert_eq!(number(block, "missing", 7.0), 7.0);
        assert_eq!(classes(&[block, "classname \"env_wind\""], "env_sky"), vec![block]);
        assert!(enabled(block) && !enabled("startdisabled true") && !enabled("enabled false"));
        assert_eq!(skyname(block).as_deref(), Some("materials/sky/x.vmat"));
        assert_eq!(resource("resource_name:\"materials/x.vpost\"", ".vpost").as_deref(), Some("materials/x.vpost"));
        assert_eq!(field(Some("m_flBloomThreshold = 4.03\nm_flBloomThresholdWidth = 9.999"), "m_flBloomThreshold"), Some(4.03));
        assert_eq!(blend_mode("m_blendMode = \"BLOOM_BLEND_SCREEN\""), Some("BLOOM_BLEND_SCREEN".into()));
        assert_eq!(biases(Some("\"g_flBrightnessExposureBias\" \"2.000\"\n")), (2.0, 0.0));
        let fog = gradient_fog(&["classname \"env_gradient_fog\"\nfogstart 1000.0\nfogcolor [ 110, 136, 160, 255 ]\nheightfog false\n"]);
        assert_eq!((fog["start"].as_f64(), fog["height"].as_bool(), fog["color"].as_array().map(Vec::len)), (Some(1000.0), Some(false), Some(3)));
    }
}
