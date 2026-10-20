//! Embed the example configurations: every file under `configs/` becomes
//! an entry of `$OUT_DIR/examples.rs`, its path there and its text, so a
//! file added to `configs/` is in the next build.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("configs");
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
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("examples.rs"),
        source,
    )
    .unwrap();
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
