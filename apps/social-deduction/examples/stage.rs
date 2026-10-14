//! Fold a game's log into the viewer's cues: the log as JSON Lines on
//! standard input, the cues as one JSON array on standard output.
//!
//! ```sh
//! cargo run -p social-deduction --example stage < game.jsonl > cues.json
//! ```

fn main() -> anyhow::Result<()> {
    let log = std::io::read_to_string(std::io::stdin())?;
    let cues = social_deduction::stage::cues(&log)?;
    serde_json::to_writer(std::io::stdout(), &cues)?;
    Ok(())
}
