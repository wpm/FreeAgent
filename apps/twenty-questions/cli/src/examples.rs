//! Twenty Questions' example configurations, embedded from `configs/` when
//! the binary is built, so a binary installed alone can write out a game to
//! play.

use free_agent_game::examples::Example;

/// Every example configuration, in order of path.
pub const EXAMPLES: &[Example] = include!(concat!(env!("OUT_DIR"), "/examples.rs"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use free_agent_game::examples::{listing, write};
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// A directory of its own for the test `test`, empty.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "twenty-questions-examples-{}-{test}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Every file in `dir`, hidden ones left out, by name. The examples
    /// sit side by side, with no folders.
    fn files(dir: &Path) -> BTreeSet<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect()
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
    fn the_listing_names_each_example_with_its_description() {
        let listing = listing(EXAMPLES);
        assert!(
            listing.lines().any(|line| line.starts_with("prompted.toml")
                && line.ends_with(
                    "Three players and a host, all one model, who picks its own secret object."
                )),
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
            config::load(&written).unwrap_or_else(|e| panic!("{}: {e:#}", example.path));
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
