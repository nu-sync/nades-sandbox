//! The workspace stands alone (docs/standalone-api.md §10): every path dependency is inside it, and no file names a
//! sibling show-me-the-nades checkout. Data comes from cs2-throws files named at run time; a comparison with
//! show-me-the-nades' web view takes that checkout as an argument (bench/web_compare.py).

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the workspace root")
}

/// Every file of the repository that is not built, generated or kept out of it.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable folder").flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if matches!(name.to_str(), Some("target" | ".git" | ".runs" | "prototypes" | "node_modules")) {
            continue;
        }
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn every_path_dependency_is_inside_the_workspace() {
    let root = root();
    let mut all = Vec::new();
    files(&root, &mut all);
    let manifests: Vec<&PathBuf> = all.iter().filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml")).collect();
    assert!(manifests.len() > 5, "the workspace's manifests");
    for manifest in manifests {
        let text = std::fs::read_to_string(manifest).unwrap();
        for (at, _) in text.match_indices("path = \"") {
            let rest = &text[at + "path = \"".len()..];
            let path = &rest[..rest.find('"').expect("a closing quote")];
            let dir = manifest.parent().unwrap();
            let target = dir.join(path).canonicalize().unwrap_or_else(|e| panic!("{}: path {path} does not resolve: {e}", manifest.display()));
            assert!(target.starts_with(&root), "{}: path {path} is outside the workspace ({})", manifest.display(), target.display());
        }
    }
}

#[test]
fn nothing_names_a_sibling_show_me_the_nades_checkout() {
    let root = root();
    let mut all = Vec::new();
    files(&root, &mut all);
    // the decision record and the proposal quote what was removed
    let records = [root.join("docs/decisions.md"), root.join("docs/standalone-api.md"), root.join(file!())];
    let patterns = ["../show-me-the-nades", "parent / \"show-me-the-nades\"", "join(\"show-me-the-nades", "\"show-me-the-nades/"];
    let mut found = Vec::new();
    for path in all.iter().filter(|p| !records.iter().any(|r| r == *p)) {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for (line, content) in text.lines().enumerate() {
            if patterns.iter().any(|p| content.contains(p)) {
                found.push(format!("{}:{}: {}", path.strip_prefix(&root).unwrap().display(), line + 1, content.trim()));
            }
        }
    }
    assert!(found.is_empty(), "files that name a sibling checkout:\n{}", found.join("\n"));
}
