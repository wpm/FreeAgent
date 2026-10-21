//! Runs the debug binary from inside an app bundle on macOS.
//!
//! `cargo tauri dev` runs the bare binary that Cargo built, not an app
//! bundle. macOS names a bare process after its executable file, so the
//! Dock tile, the application menu's title and its About, Hide and Quit
//! items all said the binary's name, as `social-deduction-desktop`. That
//! file name is the Cargo package (or `[[bin]]`) name, and Cargo passes it
//! to rustc as the crate name, which cannot contain a space, so no
//! `Cargo.toml` setting can spell the product's, as `Social Deduction`.
//! Tauri's `mainBinaryName` only renames the binary for `tauri build`, and
//! an Info.plist embedded in the binary's
//! `__info_plist` section is read by Foundation but not by the Launch
//! Services database that names apps.
//!
//! What macOS does honor is a bundle: run `X.app/Contents/MacOS/X` and
//! the name and icon come from `X.app/Contents/Info.plist`. So before
//! Tauri starts, the debug binary writes a minimal bundle next to itself
//! in the target directory, copies itself in, and replaces itself with
//! the copy. Anything that goes wrong is reported and the bare binary
//! runs as before. Release builds skip all of this; `cargo tauri build`
//! makes the real bundle.

use std::{
    env, fs, io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

/// What the bundle's Info.plist says about the app, and its icon.
pub struct App<'a> {
    /// The product's name, which names the bundle and its executable.
    pub name: &'a str,
    /// The app's version.
    pub version: &'a str,
    /// The app's identifier, as `io.github.wpm.social-deduction`.
    pub identifier: &'a str,
    /// The bytes of the `.icns` icon the Dock and the About panel show.
    pub icon: &'a [u8],
}

/// Re-executes the current binary from inside an app bundle named after
/// the product, returning only when already inside one or when the bundle
/// could not be made or started.
pub fn relaunch_in_bundle(app: &App) {
    let exe = match env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("Running unbundled: the executable's path is unknown: {error}");
            return;
        }
    };
    if exe
        .ancestors()
        .any(|ancestor| ancestor.extension().is_some_and(|ext| ext == "app"))
    {
        return;
    }
    let bundled = match write_bundle(&exe, app) {
        Ok(bundled) => bundled,
        Err(error) => {
            eprintln!("Running unbundled: could not make a dev app bundle: {error}");
            return;
        }
    };
    let error = Command::new(&bundled).args(env::args_os().skip(1)).exec();
    eprintln!(
        "Running unbundled: could not start {}: {error}",
        bundled.display()
    );
}

/// Writes `<name>.app` beside `exe` with `exe` as its executable and
/// returns the path of that executable.
///
/// The binary is copied only when it differs from the copy already there,
/// judged by size and modification time; the copy is stamped with the
/// original's modification time so the comparison holds on the next run.
fn write_bundle(exe: &Path, app: &App) -> io::Result<PathBuf> {
    let target_dir = exe
        .parent()
        .ok_or_else(|| io::Error::other("the executable has no parent directory"))?;
    let contents = target_dir
        .join(format!("{}.app", app.name))
        .join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    fs::write(contents.join("Info.plist"), info_plist(app))?;
    fs::write(resources.join("icon.icns"), app.icon)?;

    let bundled = macos.join(app.name);
    let source = fs::metadata(exe)?;
    let up_to_date = fs::metadata(&bundled).is_ok_and(|copy| {
        copy.len() == source.len()
            && match (copy.modified(), source.modified()) {
                (Ok(copy), Ok(source)) => copy == source,
                _ => false,
            }
    });
    if !up_to_date {
        fs::copy(exe, &bundled)?;
        if let Ok(modified) = source.modified() {
            fs::File::options()
                .write(true)
                .open(&bundled)?
                .set_modified(modified)?;
        }
    }
    Ok(bundled)
}

/// The smallest Info.plist that gets the app its name and icon.
fn info_plist(app: &App) -> String {
    let name = escape(app.name);
    let version = escape(app.version);
    let identifier = escape(app.identifier);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>{name}</string>
    <key>CFBundleExecutable</key>
    <string>{name}</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundleIdentifier</key>
    <string>{identifier}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#
    )
}

/// Escapes text for an XML element's content.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
