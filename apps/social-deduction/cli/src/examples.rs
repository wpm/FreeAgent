//! Social Deduction's example configurations, embedded from `configs/`
//! when the binary is built, so a binary installed alone can write out a
//! game to play.

use free_agent_game::examples::Example;

/// Every example configuration, in order of path.
pub const EXAMPLES: &[Example] = include!(concat!(env!("OUT_DIR"), "/examples.rs"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, Settled};
    use free_agent_game::examples::{listing, write};
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;
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
        let listing = listing(EXAMPLES);
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
    fn writing_makes_files_identical_to_the_sources_that_each_load() {
        let dir = scratch("identical");
        write(EXAMPLES, &dir, false).unwrap();
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
}
