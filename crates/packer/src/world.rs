//! Which of the map's meshes the game lights by its lightmap. The export cannot say: Source 2 Viewer renames a mesh's
//! lightmap stream (`texcoord` 3) to the next free TEXCOORD, so a mesh with one material UV set and a lightmap has the
//! same attributes as one with two material UV sets and none, and glTF-Transform stores both in 16 bits. The game's
//! answer is each world-node mesh's draw calls' `m_bHasBakedLightingFromLightMap` (in its `MDAT` block), which
//! Source 2 Viewer's CLI prints. A mesh without it is lit by the light probes.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, ensure};

/// For each world-node mesh under `prefix` (e.g. `maps/de_ancient`) in `vpk`: whether any of its draw calls is
/// lightmapped. `cli` is Source2Viewer-CLI.
pub fn lightmapped_meshes(cli: &Path, vpk: &Path, prefix: &str) -> Result<HashMap<String, bool>> {
    let output = Command::new(cli)
        .args(["-i"])
        .arg(vpk)
        .args(["-f", &format!("{prefix}/worldnodes/"), "-e", "vmdl_c", "-b", "MDAT"])
        // the CLI writes exceptions.txt where it runs
        .current_dir(std::env::temp_dir())
        .output()
        .with_context(|| format!("running {}", cli.display()))?;
    ensure!(output.status.success(), "{} exited {}", cli.display(), output.status);
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// The CLI's dump: `[k/n] maps/…/worldnodes/<mesh>.vmdl_c` lines, each followed by that file's blocks.
pub fn parse(dump: &str) -> HashMap<String, bool> {
    let mut meshes = HashMap::new();
    let mut current: Option<String> = None;
    for line in dump.lines() {
        let line = line.trim();
        if line.starts_with('[')
            && let Some(path) = line.split_once("] ").map(|(_, p)| p.trim())
            && let Some(stem) = path.strip_suffix(".vmdl_c")
        {
            let stem = stem.rsplit('/').next().unwrap_or(stem).to_string();
            meshes.insert(stem.clone(), false);
            current = Some(stem);
        } else if line == "m_bHasBakedLightingFromLightMap = true"
            && let Some(stem) = &current
        {
            meshes.insert(stem.clone(), true);
        }
    }
    meshes
}

/// The world-node mesh a glTF node of the export draws: `<mesh>_fragment<n>` (an aggregate's fragment) or
/// `<mesh>.meshset_<n>`.
pub fn mesh_of(node: &str) -> &str {
    if let Some((mesh, rest)) = node.rsplit_once("_fragment")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return mesh;
    }
    if let Some((mesh, rest)) = node.rsplit_once(".meshset_")
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return mesh;
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dump_says_which_meshes_have_lightmapped_draw_calls() {
        let dump = "--- Files in package:\n[2/3] maps/x/worldnodes/n0_lr0_agg_merge_wall_0.vmdl_c\n--- Data for block \"MDAT\" ---\n\t\tm_bHasBakedLightingFromLightMap = true\n[3/3] maps/x/worldnodes/n0_lr0_agg_merge_wall_1.vmdl_c\n\t\tm_bUseCompressedNormalTangent = true\n";
        let meshes = parse(dump);
        assert_eq!(meshes.get("n0_lr0_agg_merge_wall_0"), Some(&true));
        assert_eq!(meshes.get("n0_lr0_agg_merge_wall_1"), Some(&false));
        assert_eq!(mesh_of("n0_lr0_agg_merge_wall_1_fragment12"), "n0_lr0_agg_merge_wall_1");
        assert_eq!(mesh_of("n0_lr0_c33_s_cb_mesh_nsh.meshset_0"), "n0_lr0_c33_s_cb_mesh_nsh");
        assert_eq!(mesh_of("unnamed_2_32938_11523.hammer_mesh_0"), "unnamed_2_32938_11523.hammer_mesh_0");
    }
}
