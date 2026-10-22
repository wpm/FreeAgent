//! Embed the example configurations, as [`free_agent_game::embed`] does.

use std::env;
use std::path::Path;

fn main() {
    free_agent_game::embed::examples(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        Path::new(&env::var("OUT_DIR").unwrap()),
    );
}
