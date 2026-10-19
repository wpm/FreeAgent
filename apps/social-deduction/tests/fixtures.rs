//! The viewer's recorded prompted games fold into the cues its screen
//! check looks for, and a log from before logs had a header still reads.

use social_deduction_cues::{Beat, Cue, Outcome, Panel, WerewolfPhase};

/// The cues of the viewer's recorded game `name`.
fn folded(name: &str) -> Vec<Cue> {
    let path = format!(
        "{}/../social-deduction-gui/fixtures/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let log = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let mut read = social_deduction::stage::recordings(&log).unwrap();
    assert!(read.problems.is_empty(), "{:?}", read.problems);
    assert_eq!(read.games.len(), 1);
    read.games.remove(0).cues
}

/// Whether some cue's beat passes `test`.
fn any(cues: &[Cue], test: impl Fn(&Beat) -> bool) -> bool {
    cues.iter().any(|cue| test(&cue.beat))
}

/// Whether the game was won.
fn won(cues: &[Cue]) -> bool {
    any(cues, |beat| {
        matches!(
            beat,
            Beat::Over {
                outcome: Outcome::Won { .. },
                ..
            }
        )
    })
}

/// Whether `beat` is a Werewolf board in `phase`.
fn in_phase(beat: &Beat, phase: WerewolfPhase) -> bool {
    matches!(beat, Beat::Panel(Panel::Werewolf(panel)) if panel.phase == phase)
}

/// Whether `beat` tells `line`.
fn tells(beat: &Beat, line: &str) -> bool {
    matches!(beat, Beat::Told(lines) if lines.iter().any(|told| told == line))
}

/// Whether some day closes with a player speaking while it closes, and the
/// night follows.
fn spoken_while_a_day_closes(cues: &[Cue]) -> bool {
    let mut spoke = false;
    let mut closing = false;
    for cue in cues {
        match &cue.beat {
            Beat::Panel(Panel::Werewolf(panel)) if panel.phase == WerewolfPhase::Night => {
                if spoke {
                    return true;
                }
                closing = false;
            }
            Beat::Panel(Panel::Werewolf(panel)) => closing = panel.closing.is_some(),
            Beat::Said { .. } => spoke |= closing,
            _ => {}
        }
    }
    false
}

#[test]
fn the_prompted_werewolf_game_has_everything_the_screen_check_plays() {
    // Talk, thinking, night and day, a day closing with speech while it
    // closes, a ratified day, a night kill and a winner.
    let cues = folded("werewolf-prompted");
    assert!(any(&cues, |beat| matches!(beat, Beat::Said { .. })));
    assert!(any(&cues, |beat| matches!(beat, Beat::Thinking { .. })));
    assert!(spoken_while_a_day_closes(&cues));
    assert!(any(&cues, |beat| in_phase(beat, WerewolfPhase::Night)));
    assert!(any(&cues, |beat| in_phase(beat, WerewolfPhase::Day)));
    assert!(any(&cues, |beat| tells(beat, "The votes stand.")));
    assert!(any(&cues, |beat| matches!(
        beat,
        Beat::Panel(Panel::Werewolf(panel)) if !panel.nights.is_empty()
    )));
    assert!(won(&cues));
}

#[test]
fn a_log_written_before_logs_had_a_header_reads_as_it_does_with_one() {
    assert_eq!(folded("avalon-format-0"), folded("avalon-prompted"));
}

#[test]
fn the_prompted_avalon_game_reaches_the_assassination() {
    let cues = folded("avalon-prompted");
    assert!(any(&cues, |beat| matches!(beat, Beat::Said { .. })));
    assert!(any(&cues, |beat| matches!(
        beat,
        Beat::Panel(Panel::Avalon(panel))
            if panel.assassination.as_ref().is_some_and(|shot| shot.target.is_some())
    )));
    assert!(won(&cues));
}
