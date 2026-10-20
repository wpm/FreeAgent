//! Fold the first game of a log into the viewer's cues: the log as JSON
//! Lines on standard input, the cues as one JSON array on standard output.
//! Each episode left out is named on standard error.
//!
//! ```sh
//! cargo run -p social-deduction --example stage < game.jsonl > cues.json
//! ```

fn main() -> anyhow::Result<()> {
    let log = std::io::read_to_string(std::io::stdin())?;
    let read = social_deduction::stage::recordings(&log)?;
    for problem in &read.problems {
        eprintln!("{problem}");
    }
    let cues = read.games.into_iter().next().map(|game| game.cues);
    serde_json::to_writer(std::io::stdout(), &cues.unwrap_or_default())?;
    Ok(())
}
