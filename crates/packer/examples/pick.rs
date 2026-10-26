//! Which draws a pixel of a lineup view sees: casts the pixel's ray through every triangle of the pack and prints the
//! nearest hits. `cargo run --release -p nades-packer --example pick -- de_ancient x,y,z pitch,yaw px,py [w,h]`

use anyhow::Result;
use glam::{Mat4, Vec3};
use nades_pack::Pack;
use nades_pack::scene::IndexFormat;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = &args[0];
    let floats = |s: &str| s.split(',').map(|v| v.parse::<f32>().unwrap()).collect::<Vec<f32>>();
    let (pos, ang, px) = (floats(&args[1]), floats(&args[2]), floats(&args[3]));
    let size = args.get(4).map_or(vec![1138.0, 640.0], |s| floats(s));
    let inputs = nades_packer::Inputs::for_map(map)?;
    let pack = Pack::open(&inputs.out)?;
    let scene = pack.scene()?;
    let inch = 0.0254;
    let eye = Vec3::new(pos[1], pos[2] + 64.0, pos[0]) * inch;
    let (p, y) = (ang[0].to_radians(), ang[1].to_radians());
    let sf = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
    let forward = Vec3::new(sf.y, sf.z, sf.x).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let fov = 73.7398f32.to_radians();
    let h = (fov / 2.0).tan();
    let aspect = size[0] / size[1];
    let (nx, ny) = (px[0] / size[0] * 2.0 - 1.0, 1.0 - px[1] / size[1] * 2.0);
    let dir = (forward + right * nx * h * aspect + up * ny * h).normalize();
    let g = pack.geometry();
    let mut hits: Vec<(f32, usize, String)> = Vec::new();
    for (i, d) in scene.all_draws().enumerate() {
        let pos_bytes = d.positions.of(g);
        let idx = d.indices.of(g);
        let index = |k: usize| match d.index_format {
            IndexFormat::U16 => usize::from(u16::from_le_bytes([idx[k * 2], idx[k * 2 + 1]])),
            IndexFormat::U32 => u32::from_le_bytes(idx[k * 4..k * 4 + 4].try_into().unwrap()) as usize,
        };
        let vertex = |v: usize| {
            let s = |c: usize| f32::from(i16::from_le_bytes([pos_bytes[v * 8 + c * 2], pos_bytes[v * 8 + c * 2 + 1]])) / 32767.0;
            Vec3::new(s(0), s(1), s(2))
        };
        for world in d.world_transforms(g) {
            let m = Mat4::from_cols_array(&world);
            for t in 0..d.index_count as usize / 3 {
                let (a, b, c) = (m.transform_point3(vertex(index(t * 3))), m.transform_point3(vertex(index(t * 3 + 1))), m.transform_point3(vertex(index(t * 3 + 2))));
                let (e1, e2) = (b - a, c - a);
                let pv = dir.cross(e2);
                let det = e1.dot(pv);
                if det.abs() < 1e-9 {
                    continue;
                }
                let tv = eye - a;
                let u = tv.dot(pv) / det;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let qv = tv.cross(e1);
                let v = dir.dot(qv) / det;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let dist = e2.dot(qv) / det;
                if dist > 0.1 {
                    let front = det > 0.0;
                    let m = &scene.materials[d.material as usize];
                    hits.push((dist, i, format!("{} [{}] material {} ({}) lightmap {:?} {}", d.name, if front { "front" } else { "back" }, m.name, m.shader, d.lightmap.map(|l| l.from), if i >= scene.draws.len() { "skybox" } else { "" })));
                }
            }
        }
    }
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (dist, i, what) in hits.iter().take(8) {
        println!("{dist:8.3} m  draw {i}: {what}");
    }
    Ok(())
}
