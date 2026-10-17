//! The example configurations, embedded from `configs/` when the binary is
//! built, so a binary installed alone can write out a game to play.

use anyhow::{Context, bail};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// An example configuration file: where it goes, as `<game>/<name>.toml`,
/// and what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Example {
    /// The file's path under the directory it is written into, its parts
    /// separated by `/`.
    pub path: &'static str,
    /// The file's text, byte for byte as in `configs/`.
    pub text: &'static str,
}

/// Every example configuration, in order of path.
pub const EXAMPLES: &[Example] = include!(concat!(env!("OUT_DIR"), "/examples.rs"));

impl Example {
    /// The first sentence of the comment the file opens with, its lines
    /// joined, if it opens with one.
    pub fn description(&self) -> Option<String> {
        let paragraph: Vec<&str> = self
            .text
            .lines()
            .map_while(|line| line.strip_prefix('#'))
            .map(str::trim)
            .take_while(|line| !line.is_empty())
            .collect();
        let paragraph = paragraph.join(" ");
        let sentence = match paragraph.find(". ") {
            Some(end) => &paragraph[..=end],
            None => &paragraph,
        };
        (!sentence.is_empty()).then(|| sentence.to_string())
    }
}

/// What `configs` prints: each example's path, one per line, followed by
/// its description, if it has one, with the descriptions lined up.
pub fn listing() -> String {
    listed(EXAMPLES)
}

/// The listing of `examples`, as [`listing`] makes it.
fn listed(examples: &[Example]) -> String {
    let width = examples
        .iter()
        .map(|example| example.path.len())
        .max()
        .unwrap_or_default();
    let mut listing = String::new();
    for example in examples {
        match example.description() {
            Some(description) => writeln!(listing, "{:width$}  {description}", example.path),
            None => writeln!(listing, "{}", example.path),
        }
        .unwrap();
    }
    listing
}

/// Write every example into `dir`, under its path there, making the
/// directories it needs. Unless `force` is on, a file already there is an
/// error that names it, and nothing is written.
pub fn write(dir: &Path, force: bool) -> anyhow::Result<()> {
    let files: Vec<_> = EXAMPLES
        .iter()
        .map(|example| (dir.join(example.path), example.text))
        .collect();
    if !force {
        for (path, _) in &files {
            if path.exists() {
                bail!("{} already exists; --force overwrites it", path.display());
            }
        }
    }
    for (path, text) in files {
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, text));
        written.with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, Settled};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// A directory of its own for the test `test`, empty.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "social-deduction-examples-{}-{test}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Every file under `dir`, hidden ones left out, by its path there
    /// with `/` between parts.
    fn files(dir: &Path) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let names = fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (path, name)
            })
            .filter(|(_, name)| !name.starts_with('.'));
        for (path, name) in names {
            if path.is_dir() {
                found.extend(
                    files(&path)
                        .into_iter()
                        .map(|file| format!("{name}/{file}")),
                );
            } else {
                found.insert(name);
            }
        }
        found
    }

    /// The `configs/` directory the examples are embedded from.
    fn configs() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("configs")
    }

    #[test]
    fn the_examples_are_every_file_in_configs_in_order() {
        let paths: Vec<&str> = EXAMPLES.iter().map(|example| example.path).collect();
        let expected: Vec<String> = files(&configs()).into_iter().collect();
        assert_eq!(paths, expected);
    }

    #[test]
    fn the_listing_names_every_example_with_its_description_lined_up() {
        let listing = listing();
        let lines: Vec<&str> = listing.lines().collect();
        assert_eq!(lines.len(), EXAMPLES.len());
        for (line, example) in lines.iter().zip(EXAMPLES) {
            assert!(line.starts_with(example.path), "{line}");
        }
        assert!(
            lines.contains(
                &"werewolf/random.toml    Every role played by the random strategy, which is \
                  always defined: a table of random players with no models."
            ),
            "{listing}"
        );
    }

    #[test]
    fn a_description_is_the_first_sentence_of_the_opening_comment() {
        let described = |text| {
            Example {
                path: "a.toml",
                text,
            }
            .description()
        };
        assert_eq!(
            described("# One line. And\n# more.\n#\n# Later.\ngame = \"werewolf\"\n"),
            Some("One line.".to_string())
        );
        assert_eq!(
            described("# Two\n# lines\ngame = \"avalon\"\n"),
            Some("Two lines".to_string())
        );
        assert_eq!(described("game = \"avalon\"\n# Late.\n"), None);
        assert_eq!(described("#\n# After a blank.\n"), None);
    }

    #[test]
    fn a_listing_of_an_example_without_a_description_is_its_path_alone() {
        let example = Example {
            path: "werewolf/bare.toml",
            text: "game = \"werewolf\"\n",
        };
        assert_eq!(example.description(), None);
        let described = Example {
            path: "werewolf/random.toml",
            text: "# Described.\n",
        };
        assert_eq!(
            listed(&[example, described]),
            "werewolf/bare.toml\nwerewolf/random.toml  Described.\n"
        );
    }

    #[test]
    fn writing_makes_files_identical_to_the_sources_that_each_load() {
        let dir = scratch("identical");
        write(&dir, false).unwrap();
        assert_eq!(files(&dir), files(&configs()));
        for example in EXAMPLES {
            let written = dir.join(example.path);
            assert_eq!(
                fs::read(&written).unwrap(),
                fs::read(configs().join(example.path)).unwrap(),
                "{}",
                example.path
            );
            let game = config::load(&written).unwrap_or_else(|e| panic!("{e:#}"));
            let named = if example.path.starts_with("werewolf/") {
                matches!(game, Settled::Werewolf(_))
            } else {
                matches!(game, Settled::Avalon(_))
            };
            assert!(named, "{}", example.path);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writing_again_is_refused_naming_the_file_unless_forced() {
        let dir = scratch("again");
        write(&dir, false).unwrap();
        let first = dir.join(EXAMPLES[0].path);
        fs::write(&first, "changed").unwrap();
        let error = write(&dir, false).unwrap_err().to_string();
        assert!(error.contains(&first.display().to_string()), "{error}");
        assert!(error.contains("--force"), "{error}");
        assert_eq!(fs::read_to_string(&first).unwrap(), "changed");
        write(&dir, true).unwrap();
        assert_eq!(fs::read_to_string(&first).unwrap(), EXAMPLES[0].text);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_written_is_an_error_naming_it() {
        let dir = scratch("blocked");
        // A file where a game's directory belongs.
        fs::create_dir_all(&dir).unwrap();
        let game = EXAMPLES[0].path.split('/').next().unwrap();
        fs::write(dir.join(game), "").unwrap();
        let error = format!("{:#}", write(&dir, true).unwrap_err());
        assert!(error.starts_with("writing "), "{error}");
        assert!(error.contains(&dir.join(EXAMPLES[0].path).display().to_string()));
        fs::remove_dir_all(&dir).unwrap();
    }
}
