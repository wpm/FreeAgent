//! An app's example configurations, which its `build.rs` embeds from its
//! `configs/` when the binary is built, so a binary installed alone can
//! write out a game to play.

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

/// What `configs` prints of `examples`: each example's path, one per line,
/// followed by its description, if it has one, with the descriptions lined
/// up.
pub fn listing(examples: &[Example]) -> String {
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

/// Write every one of `examples` into `dir`, under its path there, making
/// the directories it needs. Unless `force` is on, a file already there is
/// an error that names it, and nothing is written.
pub fn write(examples: &[Example], dir: &Path, force: bool) -> anyhow::Result<()> {
    let files: Vec<_> = examples
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
    use std::path::PathBuf;

    /// A directory of its own for the test `test`, empty.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "free-agent-game-examples-{}-{test}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Two examples of one game and one of another.
    const EXAMPLES: &[Example] = &[
        Example {
            path: "chess/bare.toml",
            text: "game = \"chess\"\n",
        },
        Example {
            path: "chess/random.toml",
            text: "# Described.\ngame = \"chess\"\n",
        },
        Example {
            path: "go/random.toml",
            text: "# Go. At random.\ngame = \"go\"\n",
        },
    ];

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
    fn the_listing_names_every_example_with_its_description_lined_up() {
        assert_eq!(
            listing(EXAMPLES),
            "chess/bare.toml\nchess/random.toml  Described.\ngo/random.toml     Go.\n"
        );
        assert_eq!(listing(&[]), "");
    }

    #[test]
    fn writing_makes_files_identical_to_the_examples() {
        let dir = scratch("identical");
        write(EXAMPLES, &dir, false).unwrap();
        for example in EXAMPLES {
            assert_eq!(
                fs::read_to_string(dir.join(example.path)).unwrap(),
                example.text,
                "{}",
                example.path
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writing_again_is_refused_naming_the_file_unless_forced() {
        let dir = scratch("again");
        write(EXAMPLES, &dir, false).unwrap();
        let first = dir.join(EXAMPLES[0].path);
        fs::write(&first, "changed").unwrap();
        let error = write(EXAMPLES, &dir, false).unwrap_err().to_string();
        assert!(error.contains(&first.display().to_string()), "{error}");
        assert!(error.contains("--force"), "{error}");
        assert_eq!(fs::read_to_string(&first).unwrap(), "changed");
        write(EXAMPLES, &dir, true).unwrap();
        assert_eq!(fs::read_to_string(&first).unwrap(), EXAMPLES[0].text);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_written_is_an_error_naming_it() {
        let dir = scratch("blocked");
        // A file where a game's directory belongs.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("chess"), "").unwrap();
        let error = format!("{:#}", write(EXAMPLES, &dir, true).unwrap_err());
        assert!(error.starts_with("writing "), "{error}");
        assert!(error.contains(&dir.join(EXAMPLES[0].path).display().to_string()));
        fs::remove_dir_all(&dir).unwrap();
    }
}
