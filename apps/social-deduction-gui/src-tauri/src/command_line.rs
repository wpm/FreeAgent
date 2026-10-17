//! **Install Command-Line Tool…** on macOS: a link at
//! `/usr/local/bin/social-deduction` to the command line inside the app's
//! bundle, so updating the app updates the command line.
//!
//! The link is made, or removed, by a shell script run with administrator
//! privileges through `osascript`, which asks for the password. An app run
//! from its disk image, or translocated by macOS to a random read-only
//! path, is asked to move to Applications first, since a link into it
//! would dangle. After installing, the user's login shell is asked which
//! `social-deduction` a terminal runs, and one that comes before the link,
//! such as a copy `curl | sh` installed, is named.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::menu::MenuItem;
use tauri::{AppHandle, Wry};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogBuilder, MessageDialogButtons, MessageDialogKind,
};

/// The menu item's id.
pub const ID: &str = "command_line";

/// The command line's name, in the bundle and on the PATH.
const NAME: &str = "social-deduction";

/// Where the link goes, a folder on macOS's default PATH.
pub const LINK: &str = "/usr/local/bin/social-deduction";

/// What an app that would leave a dangling link is told.
const MOVE: &str = "Move Social Deduction to Applications first.";

/// What is at the link's path.
#[derive(Debug, PartialEq, Eq)]
pub enum Linked {
    /// A link into this app.
    Here,
    /// Something else: another app's link, or a copy.
    Other,
    /// Nothing.
    Absent,
}

/// What `link` is, for the app at `bundle`.
pub fn linked(link: &Path, bundle: &Path) -> Linked {
    if fs::symlink_metadata(link).is_err() {
        return Linked::Absent;
    }
    match fs::read_link(link) {
        Ok(target) if target.starts_with(bundle) => Linked::Here,
        _ => Linked::Other,
    }
}

/// The menu item's title when the link is `linked`.
pub fn title(linked: &Linked) -> &'static str {
    match linked {
        Linked::Here => "Uninstall Command-Line Tool…",
        Linked::Other | Linked::Absent => "Install Command-Line Tool…",
    }
}

/// The app bundle the executable `exe` runs in: the `.app` folder holding
/// `Contents/MacOS/<exe>`.
pub fn bundle(exe: &Path) -> Option<&Path> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then_some(bundle)
}

/// The command line inside `bundle`, which the link points to.
pub fn target(bundle: &Path) -> PathBuf {
    bundle.join("Contents/MacOS").join(NAME)
}

/// Whether a link into the app at `bundle` would last: not when it is on
/// a disk image, or macOS has translocated it.
pub fn lasting(bundle: &Path) -> bool {
    let translocated = bundle
        .components()
        .any(|component| component.as_os_str() == "AppTranslocation");
    !translocated && !bundle.starts_with("/Volumes")
}

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// The script that links `link` to `target`, making its folder if need be.
pub fn install_script(target: &Path, link: &Path) -> String {
    let folder = link.parent().unwrap_or(Path::new("/"));
    format!(
        "mkdir -p {} && ln -sfn {} {}",
        quoted(folder),
        quoted(target),
        quoted(link)
    )
}

/// The script that removes `link`.
pub fn uninstall_script(link: &Path) -> String {
    format!("rm -f {}", quoted(link))
}

/// How running a script went.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It ran.
    Done,
    /// The user cancelled the password prompt.
    Cancelled,
    /// It failed, saying this.
    Failed(String),
}

/// The outcome of `osascript`, from whether it `succeeded` and its
/// `stderr`. Cancelling the password prompt is error -128.
pub fn outcome(succeeded: bool, stderr: &str) -> Outcome {
    if succeeded {
        Outcome::Done
    } else if stderr.contains("(-128)") {
        Outcome::Cancelled
    } else {
        Outcome::Failed(stderr.trim().to_string())
    }
}

/// Run `script` with administrator privileges, asking for the password.
pub fn privileged(script: &str) -> Outcome {
    let ran = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "do shell script (item 1 of argv) with administrator privileges",
            "-e",
            "end run",
            script,
        ])
        .output();
    match ran {
        Ok(output) => outcome(
            output.status.success(),
            &String::from_utf8_lossy(&output.stderr),
        ),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// Link `link` to the command line in `bundle`, running the script with
/// `run`.
pub fn install(bundle: &Path, link: &Path, run: impl FnOnce(&str) -> Outcome) -> Outcome {
    run(&install_script(&target(bundle), link))
}

/// Remove `link`, running the script with `run`, if it points into
/// `bundle`. Anything else is left alone.
pub fn uninstall(bundle: &Path, link: &Path, run: impl FnOnce(&str) -> Outcome) -> Outcome {
    match linked(link, bundle) {
        Linked::Here => run(&uninstall_script(link)),
        Linked::Other | Linked::Absent => Outcome::Done,
    }
}

/// The paths of the commands that `type -a` lists in `output`, in the
/// order a shell tries them.
pub fn found(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .filter_map(|line| line.split_once(" is /"))
        .map(|(_, rest)| PathBuf::from(format!("/{rest}")))
        .collect()
}

/// Where the copy at `path` likely came from.
fn origin(path: &Path) -> Option<&'static str> {
    let path = path.to_string_lossy();
    if path.contains("/.cargo/bin/") {
        Some("The command-line installer (`curl | sh`) or `cargo install` put it there.")
    } else if path.starts_with("/opt/homebrew/") || path.starts_with("/usr/local/Cellar/") {
        Some("Homebrew put it there.")
    } else {
        None
    }
}

/// What to tell the user when the first `social-deduction` that `type -a`
/// lists in `output` isn't `link`. Nothing when it is, or when none is
/// listed.
pub fn shadowing(output: &str, link: &Path) -> Option<String> {
    let first = found(output).into_iter().next()?;
    if first == link {
        return None;
    }
    let origin = origin(&first)
        .map(|origin| format!(" {origin}"))
        .unwrap_or_default();
    Some(format!(
        "A terminal runs {} instead of the app's {}, since it comes first on your PATH.{origin} Remove it to run the app's.",
        first.display(),
        link.display()
    ))
}

/// What the user's shell's `type -a` says of `social-deduction`. The
/// app's own PATH is launchd's, not the shell's, and a terminal's shell is
/// a login shell that also reads `~/.zshrc` or `~/.bashrc`, where
/// installers often add to the PATH.
fn search() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    Command::new(shell)
        .args(["-ilc", &format!("type -a {NAME}")])
        .stdin(Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

/// The menu item, kept as the app's state for [`chosen`] to retitle.
pub struct Item(pub MenuItem<Wry>);

/// The bundle this app runs in, if it runs in one.
fn here() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundle(&exe).map(Path::to_path_buf)
}

/// The menu item, titled for the link's state.
///
/// # Errors
///
/// When the menu item cannot be made.
pub fn item(app: &AppHandle) -> tauri::Result<MenuItem<Wry>> {
    let state = here().map_or(Linked::Absent, |bundle| linked(Path::new(LINK), &bundle));
    MenuItem::with_id(app, ID, title(&state), true, None::<&str>)
}

/// A dialog telling the user `message`.
fn dialog(app: &AppHandle, message: impl Into<String>) -> MessageDialogBuilder<Wry> {
    app.dialog().message(message).title("Command-Line Tool")
}

/// Tell the user `message`, of `kind`.
fn tell(app: &AppHandle, kind: MessageDialogKind, message: impl Into<String>) {
    dialog(app, message).kind(kind).blocking_show();
}

/// Whether the user goes ahead after being told `message`, with `action`
/// as the button.
fn confirmed(app: &AppHandle, message: &str, action: &str) -> bool {
    dialog(app, message)
        .buttons(MessageDialogButtons::OkCancelCustom(
            action.into(),
            "Cancel".into(),
        ))
        .blocking_show()
}

/// **Install** or **Uninstall Command-Line Tool…**, by the link's state
/// now, retitling `item` after. Dialogs block, so it runs off the main
/// thread.
pub fn chosen(app: &AppHandle, item: &Item) {
    let Some(bundle) = here().filter(|bundle| lasting(bundle)) else {
        return tell(app, MessageDialogKind::Error, MOVE);
    };
    let link = Path::new(LINK);
    let password = "You'll be asked for an administrator password.";
    let state = linked(link, &bundle);
    let (message, action) = match state {
        Linked::Here => (
            format!("Removes the link to this app from /usr/local/bin. {password}"),
            "Uninstall",
        ),
        Linked::Other => (
            format!(
                "Links {NAME} in /usr/local/bin to this app, replacing the one there. {password}"
            ),
            "Install",
        ),
        Linked::Absent => (
            format!("Links {NAME} in /usr/local/bin to this app. {password}"),
            "Install",
        ),
    };
    if !confirmed(app, &message, action) {
        return;
    }
    let installing = state != Linked::Here;
    let outcome = if installing {
        install(&bundle, link, privileged)
    } else {
        uninstall(&bundle, link, privileged)
    };
    match outcome {
        Outcome::Done if installing => {
            let message = shadowing(&search(), link)
                .unwrap_or_else(|| format!("A new terminal runs {NAME}."));
            tell(app, MessageDialogKind::Info, message);
        }
        Outcome::Failed(failure) => tell(app, MessageDialogKind::Error, failure),
        Outcome::Done | Outcome::Cancelled => {}
    }
    if let Err(error) = item.0.set_text(title(&linked(link, &bundle))) {
        tracing::warn!("the Command-Line Tool menu item could not be retitled: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A new, empty folder for the test `name`.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("command-line-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run `script` with `sh`, as `osascript` would but without privileges.
    fn sh(script: &str) -> Outcome {
        let output = Command::new("sh").args(["-c", script]).output().unwrap();
        outcome(
            output.status.success(),
            &String::from_utf8_lossy(&output.stderr),
        )
    }

    #[test]
    fn the_bundle_is_the_app_folder_around_the_executable() {
        let exe =
            Path::new("/Applications/Social Deduction.app/Contents/MacOS/social-deduction-desktop");
        assert_eq!(
            bundle(exe),
            Some(Path::new("/Applications/Social Deduction.app"))
        );
        assert_eq!(bundle(Path::new("/usr/bin/social-deduction-desktop")), None);
        assert_eq!(bundle(Path::new("/target/debug/MacOS/x")), None);
        assert_eq!(bundle(Path::new("x")), None);
    }

    #[test]
    fn the_link_points_to_the_command_line_in_the_bundle() {
        assert_eq!(
            target(Path::new("/Applications/Social Deduction.app")),
            Path::new("/Applications/Social Deduction.app/Contents/MacOS/social-deduction")
        );
    }

    #[test]
    fn an_app_on_a_disk_image_or_translocated_is_refused() {
        let translocated = Path::new(
            "/private/var/folders/xy/abc/T/AppTranslocation/1234-5678/d/Social Deduction.app",
        );
        assert!(!lasting(translocated));
        assert!(!lasting(Path::new(
            "/Volumes/Social Deduction/Social Deduction.app"
        )));
        assert!(lasting(Path::new("/Applications/Social Deduction.app")));
        assert!(lasting(Path::new(
            "/Users/me/Applications/Social Deduction.app"
        )));
    }

    #[test]
    fn the_menu_offers_install_or_uninstall_by_the_link() {
        let dir = folder("menu");
        let bundle = dir.join("Social Deduction.app");
        let link = dir.join("bin/social-deduction");
        assert_eq!(linked(&link, &bundle), Linked::Absent);
        assert_eq!(title(&Linked::Absent), "Install Command-Line Tool…");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(target(&bundle), &link).unwrap();
        assert_eq!(linked(&link, &bundle), Linked::Here);
        assert_eq!(title(&Linked::Here), "Uninstall Command-Line Tool…");
        let other = dir.join("Other.app");
        assert_eq!(linked(&link, &other), Linked::Other);
        assert_eq!(title(&Linked::Other), "Install Command-Line Tool…");
        fs::remove_file(&link).unwrap();
        fs::write(&link, "a copy").unwrap();
        assert_eq!(linked(&link, &bundle), Linked::Other);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installing_links_into_the_bundle_making_the_folder() {
        let dir = folder("install it's");
        let bundle = dir.join("Social Deduction.app");
        let link = dir.join("usr local/bin/social-deduction");
        assert_eq!(install(&bundle, &link, sh), Outcome::Done);
        assert_eq!(fs::read_link(&link).unwrap(), target(&bundle));
        let newer = dir.join("Newer.app");
        assert_eq!(install(&newer, &link, sh), Outcome::Done);
        assert_eq!(fs::read_link(&link).unwrap(), target(&newer));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn uninstalling_removes_only_a_link_into_this_app() {
        let dir = folder("uninstall");
        let bundle = dir.join("Social Deduction.app");
        let link = dir.join("social-deduction");
        let refused = |_: &str| -> Outcome { panic!("nothing runs") };
        assert_eq!(uninstall(&bundle, &link, refused), Outcome::Done);
        symlink(target(&dir.join("Other.app")), &link).unwrap();
        assert_eq!(uninstall(&bundle, &link, refused), Outcome::Done);
        assert!(fs::symlink_metadata(&link).is_ok());
        fs::remove_file(&link).unwrap();
        symlink(target(&bundle), &link).unwrap();
        assert_eq!(uninstall(&bundle, &link, sh), Outcome::Done);
        assert!(fs::symlink_metadata(&link).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_cancelled_password_prompt_is_not_a_failure() {
        assert_eq!(outcome(true, ""), Outcome::Done);
        assert_eq!(
            outcome(false, "0:85: execution error: User canceled. (-128)\n"),
            Outcome::Cancelled
        );
        assert_eq!(
            outcome(false, "0:85: execution error: ln: Permission denied (1)\n"),
            Outcome::Failed("0:85: execution error: ln: Permission denied (1)".to_string())
        );
    }

    #[test]
    fn the_first_copy_on_the_path_is_named_when_it_is_not_the_link() {
        let link = Path::new(LINK);
        let ours = "social-deduction is /usr/local/bin/social-deduction\n";
        assert_eq!(shadowing(ours, link), None);
        assert_eq!(shadowing("", link), None);
        assert_eq!(shadowing("social-deduction not found\n", link), None);
        let cargo = "social-deduction is /Users/me/.cargo/bin/social-deduction\n\
                     social-deduction is /usr/local/bin/social-deduction\n";
        let told = shadowing(cargo, link).unwrap();
        assert!(told.starts_with("A terminal runs /Users/me/.cargo/bin/social-deduction"));
        assert!(told.contains("`curl | sh`"));
        let brew = "social-deduction is /opt/homebrew/bin/social-deduction\n";
        assert!(shadowing(brew, link).unwrap().contains("Homebrew"));
        let elsewhere = "social-deduction is /Users/me/bin/social-deduction\n";
        assert_eq!(
            shadowing(elsewhere, link).unwrap(),
            "A terminal runs /Users/me/bin/social-deduction instead of the app's \
             /usr/local/bin/social-deduction, since it comes first on your PATH. \
             Remove it to run the app's."
        );
    }

    #[test]
    fn every_copy_type_lists_is_found_in_order() {
        let output = "social-deduction is an alias for echo\n\
                      social-deduction is /a b/social-deduction\n\
                      social-deduction is /usr/local/bin/social-deduction\n";
        assert_eq!(
            found(output),
            [PathBuf::from("/a b/social-deduction"), PathBuf::from(LINK)]
        );
    }
}
