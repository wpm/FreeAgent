//! What an app's `build.rs` runs to embed its example configurations:
//! every file under its `configs/` becomes an entry of `examples.rs` in
//! its `OUT_DIR`, its path there and its text, so a file added to
//! `configs/` is in the next build. The app includes that file as its
//! slice of `Example`s.
//!
//! A build script depends on this crate without its default features, so
//! that it builds this module alone.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Embed every file under `configs/` in `manifest_dir`, hidden ones left
/// out, into `examples.rs` in `out_dir`, in order of path, and have Cargo
/// build again when `configs/` changes.
///
/// # Panics
///
/// When `configs/` cannot be read or `examples.rs` cannot be written,
/// which fails the build.
pub fn examples(manifest_dir: &Path, out_dir: &Path) {
    let root = manifest_dir.join("configs");
    println!("cargo::rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    let mut source = String::from("&[\n");
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        writeln!(
            source,
            "    Example {{ path: {:?}, text: include_str!({:?}) }},",
            name,
            file.display().to_string()
        )
        .unwrap();
    }
    source.push_str("]\n");
    fs::write(out_dir.join("examples.rs"), source).unwrap();
}

/// Every file under `dir`, hidden ones left out, into `files`.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(&path, files);
        } else {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_under_configs_but_hidden_ones_is_embedded_in_order_of_path() {
        let dir = std::env::temp_dir().join(format!("free-agent-embed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let configs = dir.join("configs");
        fs::create_dir_all(configs.join("werewolf")).unwrap();
        for file in ["werewolf/random.toml", "mixed.toml", ".hidden.toml"] {
            fs::write(configs.join(file), "").unwrap();
        }
        examples(&dir, &dir);
        let entry = |path: &str| {
            let file = path.split('/').fold(configs.clone(), |file, part| file.join(part));
            let file = file.display().to_string();
            format!("    Example {{ path: {path:?}, text: include_str!({file:?}) }},\n")
        };
        assert_eq!(
            fs::read_to_string(dir.join("examples.rs")).unwrap(),
            format!(
                "&[\n{}{}]\n",
                entry("mixed.toml"),
                entry("werewolf/random.toml")
            )
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
