//! The types Social Deduction's native code shares with its WASM frontend.
//!
//! The frontend builds for `wasm32-unknown-unknown`, so it cannot depend on
//! `social-deduction`, which needs Tokio, reqwest and the keychain. A type
//! or helper belongs here when the frontend reads, produces or checks it
//! and the native side has to agree with it. Everything the frontend does
//! not touch stays in `social-deduction`.
//!
//! The viewer shows one Werewolf or Avalon game in God mode, as a group chat
//! beside a live board. Library code folds the game's log into a stream of
//! [`Cue`]s, and the frontend draws each one as it arrives. These types are
//! everything that crosses between the two, and they build for
//! `wasm32-unknown-unknown`, so the frontend shares them. A log's games
//! cross as [`Recordings`], and the desktop app's [`settings`] and the
//! [`go`] menu's moves cross the same way.

#![warn(missing_docs)]

pub mod go;
pub mod settings;

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A player's name, `Player 3`, which keys a seat everywhere on screen.
pub type PlayerId = String;

/// Milliseconds since the Unix epoch, as the log stamps events. Plain
/// integers, because `SystemTime::now()` panics in wasm.
pub type Millis = u64;

/// One thing for the screen to show, stamped with the log time it happened
/// at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    /// When it happened, in log time.
    pub at: Millis,
    /// What happened.
    pub beat: Beat,
}

/// A log's episodes, each read as its own game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Recordings {
    /// Every episode that reads, in the order of each one's first line.
    pub games: Vec<Recording>,
    /// Why each episode left out was left out, naming the line.
    pub problems: Vec<String>,
}

/// One episode of a log, folded into cues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// The episode's ID, which every one of its lines carries.
    pub episode: Uuid,
    /// The game it plays.
    pub game: GameName,
    /// Its cues, the last of them its one [`Beat::Over`].
    pub cues: Vec<Cue>,
}

/// A game log opened for review: the file's name and its games.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    /// The file's name, as the header shows it.
    pub name: String,
    /// The log's games.
    pub recordings: Recordings,
}

/// What a cue shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Beat {
    /// The deal: every seat in seat order. Always first. The screen seats
    /// evil players on the left and good players on the right.
    Cast {
        /// The game being played.
        game: GameName,
        /// Every seat, in seat order.
        seats: Vec<Seat>,
    },
    /// How fast log time runs against the viewer's clock: 1.0 live. Sent
    /// right after `Cast`, and again on any change (playback speed, later).
    Clock {
        /// Log milliseconds per viewer millisecond.
        rate: f64,
    },
    /// A chapter heading: `Night 2.`
    Heading(String),
    /// What one narrator call told, blank lines and votes dropped: one
    /// centered block.
    Told(Vec<String>),
    /// An utterance, shown as a bubble from its speaker's seat.
    Said {
        /// Who spoke.
        who: PlayerId,
        /// What they said.
        text: String,
    },
    /// A player's model call went out: show its thinking dots.
    Thinking {
        /// Whose call went out.
        who: PlayerId,
    },
    /// The player's call is over, however it ended: take the dots away.
    Thought {
        /// Whose call is over.
        who: PlayerId,
    },
    /// The game board as it stands now, whole. It replaces the board shown.
    Panel(Panel),
    /// The game is over, however it ended. Always last.
    Over {
        /// How it ended.
        outcome: Outcome,
    },
}

/// Which game is being played, which picks the board to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameName {
    /// Werewolf.
    Werewolf,
    /// Avalon.
    Avalon,
}

impl GameName {
    /// Every game, in the order the app lists them.
    pub const ALL: [GameName; 2] = [GameName::Werewolf, GameName::Avalon];

    /// What the game is, in a sentence or two.
    pub fn description(self) -> &'static str {
        match self {
            GameName::Werewolf => {
                "A village hides its werewolves. Each night the wolves kill, the seer learns one \
                 player’s nature and the doctor saves one life. Each day the village argues and \
                 votes someone out."
            }
            GameName::Avalon => {
                "Arthur’s servants send teams on quests; Mordred’s minions slip onto them and \
                 sabotage. Three good quests win it, unless the Assassin can name Merlin."
            }
        }
    }
}

/// A side of the game: good players speak from the right, evil from the
/// left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// The villagers, or Arthur's servants.
    Good,
    /// The werewolves, or Mordred's minions.
    Evil,
}

/// How a game ended, shown at the foot of the chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// A side won.
    Won {
        /// The side that won.
        side: Side,
        /// How it won, as the narrator tells it.
        how: String,
    },
    /// The game was stopped before it ended.
    Stopped,
    /// The game failed, for the reason given.
    Failed(String),
    /// The log ends before the game does.
    Unfinished,
}

/// One player at the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    /// The player's name: `Player 3`.
    pub name: PlayerId,
    /// The name with its role, `Player 3 · Seer`: the bubble's name and its
    /// key in the list of names.
    pub label: String,
    /// The player's role, which the board names, and whose side seats
    /// them left or right and colors their marks.
    pub role: Role,
    /// The strategy the player was dealt.
    pub strategy: String,
}

/// Every role of every game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// Werewolf's villager.
    Villager,
    /// Werewolf's seer.
    Seer,
    /// Werewolf's doctor.
    Doctor,
    /// Werewolf's werewolf.
    Werewolf,
    /// Avalon's Merlin.
    Merlin,
    /// Avalon's Percival.
    Percival,
    /// Avalon's loyal servant of Arthur.
    Servant,
    /// Avalon's Assassin.
    Assassin,
    /// Avalon's Morgana.
    Morgana,
    /// Avalon's Mordred.
    Mordred,
    /// Avalon's Oberon.
    Oberon,
    /// Avalon's minion of Mordred.
    Minion,
}

impl Role {
    /// Every role, Werewolf's then Avalon's.
    pub const ALL: [Role; 12] = [
        Role::Villager,
        Role::Seer,
        Role::Doctor,
        Role::Werewolf,
        Role::Merlin,
        Role::Percival,
        Role::Servant,
        Role::Assassin,
        Role::Morgana,
        Role::Mordred,
        Role::Oberon,
        Role::Minion,
    ];

    /// The side the role plays on.
    pub fn side(self) -> Side {
        match self {
            Role::Villager
            | Role::Seer
            | Role::Doctor
            | Role::Merlin
            | Role::Percival
            | Role::Servant => Side::Good,
            Role::Werewolf
            | Role::Assassin
            | Role::Morgana
            | Role::Mordred
            | Role::Oberon
            | Role::Minion => Side::Evil,
        }
    }

    /// The role whose settings key is `key`: `seer`, `merlin`.
    pub fn from_key(key: &str) -> Option<Role> {
        Role::ALL
            .into_iter()
            .find(|role| role.to_string().eq_ignore_ascii_case(key))
    }
}

/// A role displays as the console names it: `seer`, `werewolf`, `Merlin`,
/// `Assassin`.
impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Villager => "villager",
            Role::Seer => "seer",
            Role::Doctor => "doctor",
            Role::Werewolf => "werewolf",
            Role::Merlin => "Merlin",
            Role::Percival => "Percival",
            Role::Servant => "Servant",
            Role::Assassin => "Assassin",
            Role::Morgana => "Morgana",
            Role::Mordred => "Mordred",
            Role::Oberon => "Oberon",
            Role::Minion => "Minion",
        })
    }
}

/// The game board, drawn whole each time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Panel {
    /// A Werewolf board.
    Werewolf(WerewolfPanel),
    /// An Avalon board.
    Avalon(AvalonPanel),
}

/// The Werewolf board: who is alive, where the vote stands, what each night
/// role did, and what each seer knows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WerewolfPanel {
    /// The round, from 1: night 1, then day 1.
    pub round: u8,
    /// Night or day, which darkens or lightens the window.
    pub phase: WerewolfPhase,
    /// When the phase began, which starts its clock.
    pub began: Millis,
    /// How long the phase may last, which the clock counts down.
    pub limit_ms: u64,
    /// The day's tentative decision, with its ratify countdown.
    pub ratify: Option<Ratify>,
    /// When the phase began closing, while it closes: its outcome is
    /// fixed, and it waits for the players still finishing what they were
    /// saying.
    pub closing: Option<Millis>,
    /// Every seat, in seat order, alive or dead.
    pub seats: Vec<SeatState>,
    /// By day: each living player's vote, `None` while undecided.
    pub votes: Vec<(PlayerId, Option<PlayerId>)>,
    /// By night: what each werewolf, doctor and seer has done.
    pub night: Vec<(PlayerId, NightDeed)>,
    /// Per seer, everything learned.
    pub seers: Vec<(PlayerId, Vec<Learned>)>,
    /// One per night resolved.
    pub nights: Vec<NightOutcome>,
}

/// A Werewolf phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WerewolfPhase {
    /// Night: the window is dark.
    Night,
    /// Day: the window is light.
    Day,
}

/// The day's tentative decision, which stands once its countdown runs out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ratify {
    /// Whom the vote would put out, or `None` for nobody.
    pub target: Option<PlayerId>,
    /// When the decision was reached, which starts its countdown.
    pub since: Millis,
    /// How long the decision must stand.
    pub limit_ms: u64,
}

/// One seat on the Werewolf board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatState {
    /// The player.
    pub name: PlayerId,
    /// When the player died, or `None` while alive.
    pub died: Option<Death>,
}

/// When a player died: killed by night, voted out by day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Death {
    /// The round of the death.
    pub round: u8,
    /// Night for a kill, day for a vote.
    pub phase: WerewolfPhase,
}

/// What one night role has done tonight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NightDeed {
    /// Nothing yet.
    Pending,
    /// A werewolf chose this player to kill.
    Kill(PlayerId),
    /// A doctor chose this player to protect.
    Protect(PlayerId),
    /// A seer asked about this player.
    Ask(PlayerId),
}

/// What a seer learned about one player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Learned {
    /// The player asked about.
    pub player: PlayerId,
    /// The player's role.
    pub role: Role,
    /// The night it was learned.
    pub night: u8,
}

/// How one night turned out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightOutcome {
    /// The night.
    pub night: u8,
    /// Whom the werewolves chose, if anyone.
    pub victim: Option<PlayerId>,
    /// The doctors who protected the victim.
    pub protected_by: Vec<PlayerId>,
    /// Who died, if anyone.
    pub died: Option<PlayerId>,
}

/// The Avalon board: the quest track, the proposals, the team on the table
/// and every vote so far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvalonPanel {
    /// The quest, from 1.
    pub quest: u8,
    /// The proposal within the quest, from 1.
    pub proposal: u8,
    /// What the table is doing now.
    pub phase: AvalonPhase,
    /// When the phase began, which starts its clock.
    pub began: Millis,
    /// How long the phase may last, which the clock counts down.
    pub limit_ms: u64,
    /// The leader, who proposes the team.
    pub leader: PlayerId,
    /// The team on the table, once proposed.
    pub team: Option<Vec<PlayerId>>,
    /// This quest's team, each with their deed once played.
    pub deeds: Vec<(PlayerId, Option<Deed>)>,
    /// The quest track: always five tokens.
    pub quests: Vec<QuestToken>,
    /// Every proposal, newest first.
    pub proposals: Vec<ProposalRow>,
    /// Every seat, in seat order.
    pub seats: Vec<AvalonSeat>,
    /// The Assassin's shot at Merlin, once the good side has won three
    /// quests.
    pub assassination: Option<Assassination>,
}

/// An Avalon phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AvalonPhase {
    /// The leader is proposing a team.
    Propose,
    /// The table is voting on the team.
    Vote,
    /// The team is on the quest.
    Quest,
    /// The Assassin is naming Merlin.
    Assassinate,
}

/// What a team member played on a quest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Deed {
    /// Helped the quest succeed.
    Support,
    /// Sabotaged the quest.
    Sabotage,
}

/// One token on the quest track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestToken {
    /// How many players go on the quest.
    pub size: usize,
    /// How many sabotages fail it.
    pub sabotages_needed: usize,
    /// How it went, once played.
    pub result: Option<QuestResult>,
}

/// How a quest went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestResult {
    /// The team that went.
    pub team: Vec<PlayerId>,
    /// How many sabotaged it.
    pub sabotages: usize,
    /// Whether it succeeded.
    pub succeeded: bool,
}

/// One proposal in the list of proposals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRow {
    /// The quest it was for.
    pub quest: u8,
    /// Its number within the quest.
    pub proposal: u8,
    /// Who proposed it.
    pub leader: PlayerId,
    /// The team proposed, once named.
    pub team: Option<Vec<PlayerId>>,
    /// Every seat's ballot, `None` while there is none.
    pub ballots: Vec<(PlayerId, Option<bool>)>,
    /// Whether the table approved it, `None` while the vote is open.
    pub approved: Option<bool>,
    /// How the quest went, once an approved team played it.
    pub result: Option<QuestResult>,
}

/// One seat on the Avalon board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvalonSeat {
    /// The player.
    pub name: PlayerId,
    /// The players this player sees.
    pub sees: Vec<PlayerId>,
}

/// The Assassin's shot at Merlin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assassination {
    /// The Assassin.
    pub assassin: PlayerId,
    /// Whom the Assassin named, once named.
    pub target: Option<PlayerId>,
    /// Whether the target was Merlin, `None` until resolved.
    pub merlin: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_game_says_what_it_is() {
        for game in GameName::ALL {
            assert!(!game.description().is_empty());
        }
    }
}
