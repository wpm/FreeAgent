# Releasing

A release is a git tag. Pushing `v0.1.0` runs the release workflow, which
builds the applications for macOS, Linux, and Windows, uploads the binaries
and their installers to a GitHub Release with that version's notes from
[CHANGELOG.md](CHANGELOG.md), and then publishes `free-agent` to crates.io.
When it succeeds, a second workflow adds the desktop app's installers.

## Cutting a release

1. Set `version` under `[workspace.package]` in `Cargo.toml` and the version
   of the `free-agent` entry under `[workspace.dependencies]` to match.
   Every crate in the workspace shares the one version.
2. Move the `Unreleased` notes in `CHANGELOG.md` under a heading for the
   version and date, and add the version's compare link at the bottom.
3. Check what will ship: `dist plan` lists the artifacts and
   `cargo publish --workspace --dry-run` packages the library.
4. Merge, then tag the merge commit on `main` and push the tag:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

5. Watch the Release workflow under Actions. It creates the GitHub Release
   when the binaries are up and publishes to crates.io after that.
6. Watch **Release the desktop app** under Actions, which starts when the
   Release workflow succeeds. It builds the desktop app's installers on
   macOS, Linux and Windows and puts them on the release, some fifteen to
   twenty minutes later.
7. Check the release page: a **Desktop app** table above the notes, and
   among the assets `Social-Deduction.dmg`, `Social-Deduction-setup.exe`,
   `Social-Deduction.msi`, and for Linux `social-deduction-amd64.deb`,
   `social-deduction-arm64.deb`, `social-deduction-x86_64.rpm`,
   `social-deduction-aarch64.rpm`, `Social-Deduction-x86_64.AppImage` and
   `Social-Deduction-aarch64.AppImage`, each beside its versioned twin,
   and `latest.json`, with entries for `darwin-aarch64`, `darwin-x86_64`,
   `linux-x86_64`, `linux-aarch64` and `windows-x86_64`.
   `https://github.com/wpm/FreeAgent/releases/latest/download/latest.json`
   serves it. A prerelease never becomes `releases/latest`, so installed
   apps are offered only full releases.
   The website links to the .dmg, the setup.exe, and the `.deb` and `.rpm`
   for each Linux architecture, and the workflow's last job, **The
   website's downloads**, checks that the release has each of them.

## The desktop app

`.github/workflows/release-gui.yml` builds the installers with
[tauri-action](https://github.com/tauri-apps/tauri-action), one job per
platform. A failed platform leaves the release without its installers:
open the run and choose **Re-run failed jobs**, which uploads them again
over any that are there. Run the workflow by hand from the Actions tab to
build every installer as a workflow artifact without touching a release;
a pull request that changes the workflow or its scripts does the same.

Every bundle carries the `social-deduction` command line that dist built
for the release, so the workflow downloads dist's archives from the
release: the Linux and Windows bundles hold dist's binaries as they are,
and the macOS app holds dist's two joined into one universal binary and
signed with the app. Each build job checks that its bundles' copy prints
the release's version. A trial run builds the command line itself, or,
given a tag in **sidecar-from**, takes it from that release.

On Windows, the setup.exe adds its folder to the user's PATH and its
uninstaller takes it off, through the hooks in
`src-tauri/installer-hooks.nsh`. The Windows job installs it, runs
`social-deduction --version` in a new shell, and uninstalls it, checking
the PATH each time. The .msi installs the app alone.

Each release carries a signed update bundle for each platform and a
`latest.json` that the installed app reads on launch to offer the update.
The bundles are signed with the minisign key in the
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
secrets, and the build fails without them. The matching public key is
`plugins.updater.pubkey` in `src-tauri/tauri.conf.json`. The key pair
cannot be recovered, and replacing it strands every installed copy, which
then has to be reinstalled by hand, so the private key and its password
are kept backed up outside this machine and repository.

The macOS app is signed and notarized when the repository has the Apple
secrets that `.github/scripts/apple-credentials.sh` names, and is unsigned
otherwise. The release notes say which, and how to open an unsigned app.

## One-time setup

The release workflow authenticates to crates.io by
[trusted publishing](https://crates.io/docs/trusted-publishing), which is
configured on a crate that already exists. So the very first release goes
like this:

1. Create the `release` environment under Settings → Environments. The
   crates.io job runs in it, so the environment can be given required
   reviewers to make publishing a deliberate click.
2. Publish the library by hand once, with a token from
   [crates.io/settings/tokens](https://crates.io/settings/tokens) that has
   the `publish-new` scope:

   ```sh
   cargo login
   cargo publish -p free-agent
   ```

3. On [crates.io/crates/free-agent/settings](https://crates.io/crates/free-agent/settings),
   add a trusted publisher: owner `wpm`, repository `FreeAgent`, workflow
   `publish-crates.yml`, environment `release`.

From the second release on, the tag does everything. The first tag still
builds the binaries and the GitHub Release; its crates.io job sees the
version already published and skips it.

## Tooling

The release workflow and `dist-workspace.toml` are generated by
[dist](https://axodotdev.github.io/cargo-dist/book/), installed with
`cargo install cargo-dist --locked`. After changing `dist-workspace.toml`,
run `dist init` to regenerate the workflow, and `dist plan` to see the
result. The workflow checks itself for staleness and fails a release whose
configuration and workflow disagree.
