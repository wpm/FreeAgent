# Releasing

Each product is released on its own, with its own version, by publishing
a release in the [Releases page](https://github.com/wpm/FreeAgent/releases).
The crates in a product share its version.

| Product | Crates | Tag | A release produces |
|---|---|---|---|
| `free-agent` | `free-agent` | `free-agent-v<version>` | `free-agent` on crates.io |
| `social-deduction` | `social-deduction`, `social-deduction-gui`, `social-deduction-desktop`, `social-deduction-cues` | `social-deduction-v<version>` | Desktop installers for macOS, Windows and Linux, command-line archives, signed update bundles and `latest.json` |

The tag is the version. Every manifest on `main` says `version = "0.0.0"`,
and the release workflow stamps the tag's version into the product's
crates with `.github/scripts/stamp-version.sh` before it builds or
publishes.

## Release a product

1. **Releases → Draft a new release.**
2. **Choose a tag:** type `<product>-v<version>`, for example
   `social-deduction-v0.2.0`, and choose **Create new tag on publish**.
   Leave **Target** as `main`.
3. **Previous tag:** choose that product's last tag, for example
   `social-deduction-v0.1.0`. GitHub may otherwise pick another product's
   tag.
4. Click **Generate release notes**. Delete lines in other categories that
   don't belong to this product. Title the release `<Product> <version>`,
   for example "Social Deduction 0.2.0".
5. For a candidate (`-rc.N`), check **Set as a pre-release**. It is built
   and uploaded, and the website and installed apps keep to the last full
   release.
6. Click **Publish release**.

Publishing runs the product's workflow under Actions:

- **Social Deduction:** **Release Social Deduction**
  (`.github/workflows/release-social-deduction.yml`), about 15–20
  minutes. It puts the desktop app's install table above the notes and
  uploads, each beside its versioned twin, `Social-Deduction.dmg`,
  `Social-Deduction-setup.exe`, `Social-Deduction.msi`,
  `social-deduction-amd64.deb`, `social-deduction-arm64.deb`,
  `social-deduction-x86_64.rpm`, `social-deduction-aarch64.rpm`,
  `Social-Deduction-x86_64.AppImage` and
  `Social-Deduction-aarch64.AppImage`; the command line's archives
  `social-deduction-universal-apple-darwin.tar.gz`,
  `social-deduction-x86_64-unknown-linux-gnu.tar.gz`,
  `social-deduction-aarch64-unknown-linux-gnu.tar.gz` and
  `social-deduction-x86_64-pc-windows-msvc.zip`, each with its `.sha256`;
  the update bundles with their `.sig`; and `latest.json`, with entries
  for `darwin-aarch64`, `darwin-x86_64`, `linux-x86_64`, `linux-aarch64`
  and `windows-x86_64`. Its last job starts the **Website** workflow,
  which points the download links at the new release and checks that it
  has each linked file. Then
  `https://wpm.github.io/FreeAgent/social-deduction/latest.json` shows
  the new version.
- **The library:** **Publish to crates.io**
  (`.github/workflows/publish-crates.yml`), about a minute. Check the
  version on [crates.io/crates/free-agent](https://crates.io/crates/free-agent).
  It runs in the `release` environment, which crates.io's trusted
  publisher for `publish-crates.yml` expects.

When a job fails, open the run and choose **Re-run failed jobs**, which
uploads over what is there.

## Choosing a version

Each product follows [semver](https://semver.org) on its own. Before 1.0,
a breaking change bumps the minor version; from 1.0 on, the major. A
feature bumps the minor version and a fix the patch. Versions live only in
tags: `main` stays at `0.0.0`, and development builds report it.

## Undoing a release

Delete the release and its tag (`git push origin :refs/tags/<tag>`), fix
`main`, and publish again with the next patch version. A library version
on crates.io stays there: `cargo yank` hides it from new dependents.

## The desktop app's signing and updates

The macOS app is signed and notarized when the repository has the Apple
secrets that `.github/scripts/apple-credentials.sh` names, and is unsigned
otherwise. The release notes say which, and how to open an unsigned app.

Each release carries a signed update bundle for each platform. They are
signed with the minisign key in the `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets, and the build fails without
them. The matching public key is `plugins.updater.pubkey` in
`apps/social-deduction-gui/src-tauri/tauri.conf.json`. Every installed
copy trusts that key for good, so the private key and its password are
kept backed up outside this machine and repository, and the key is never
replaced.

The installed app looks for updates at its updater endpoint,
`https://wpm.github.io/FreeAgent/social-deduction/latest.json`, which the
website's build copies from the newest full release. Every installed copy
keeps the endpoint it was built with.

## Adding an application

1. List its crates under a new product in the table in
   `.github/scripts/stamp-version.sh`.
2. Copy `.github/workflows/release-social-deduction.yml` with the new tag
   prefix.
3. Add a label for it to `.github/labeler.yml`, create the label, and add
   a category for it to `.github/release.yml`.
4. Link its downloads on the website as
   `{{release:<product>}}/<file>`, which the website's build resolves to
   its newest full release. The build needs one, so these links go in
   with or after its first full release.

Its first release then needs no other setup.
