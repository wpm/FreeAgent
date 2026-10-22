use std::path::Path;

fn main() {
    if require_sidecar() {
        tauri_build::build()
    }
}

/// Whether the `twenty-questions` command line is where the bundle picks
/// it up as a sidecar (`bundle.externalBin` in `tauri.conf.json`), under
/// Tauri's name for it, which `.github/scripts/sidecar.sh twenty-questions`
/// also gives it: `binaries/twenty-questions-<target triple>`, with `.exe`
/// on Windows.
/// When it is not, the build fails with an error saying how to make it.
fn require_sidecar() -> bool {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for a build script");
    let exe = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let sidecar = Path::new("binaries").join(format!("twenty-questions-{target}{exe}"));
    let built = sidecar.exists();
    if !built {
        println!(
            "cargo::error=The desktop app's sidecar, src-tauri/{}, is missing. \
             Make it by running .github/scripts/sidecar.sh twenty-questions from the root of \
             the checkout.",
            sidecar.display()
        );
    }
    built
}
