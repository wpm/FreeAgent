//! What the screen shows, as a pure function of the cues so far.
//!
//! [`Screen::apply`] folds one cue at a time into the chat, the names
//! table, the header, the clock and the board. The view draws a `Screen`
//! and holds no state of its own, so the whole viewer is tested here,
//! natively.

use leptos_rich_chat::{Look, Names, Position};
use social_deduction_stage::{
    AvalonPanel, Beat, Cue, Death, GameName, Learned, Millis, NightDeed, NightOutcome, Outcome,
    Panel, PlayerId, Role, Seat, Side, WerewolfPanel, WerewolfPhase,
};

/// The name narration is posted under, centered between the sides.
pub const NARRATOR: &str = "Narrator";

/// How long, in log time, a model call is out before its dots show, so
/// that a call that ends at once does not flicker.
pub const THINKING_DELAY: Millis = 300;

/// One message in the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    /// `m1`, `m2`, … in arrival order.
    pub id: String,
    /// Who it is from: a seat's label, or [`NARRATOR`].
    pub name: String,
    /// What it shows.
    pub body: Body,
}

/// What a message shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// A chapter heading, drawn as a centered rule.
    Heading(String),
    /// Markdown, drawn as a bubble.
    Text(String),
    /// Markdown said too late to be sent, drawn as a faded bubble.
    Late(String),
}

/// A player's thinking dots, pinned below the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thinker {
    /// The player.
    pub who: PlayerId,
    /// The player's label.
    pub name: String,
}

/// A choice on the board that has just been made, which flashes as it
/// lands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Landed {
    /// The team on the table, as its leader proposed it.
    Team,
    /// A player's ballot on a proposal.
    Ballot {
        /// The quest the proposal is for.
        quest: u8,
        /// The proposal's number within its quest.
        proposal: u8,
        /// Who voted.
        player: PlayerId,
    },
    /// A team member's deed.
    Deed(PlayerId),
    /// The Assassin's target.
    Target,
    /// A Werewolf player's vote, or its retraction.
    Vote(PlayerId),
    /// A Werewolf night role's deed.
    Night(PlayerId),
    /// The day's tentative decision.
    Ratify,
    /// A Werewolf player's death.
    Death(PlayerId),
}

/// Log time as the viewer reads it: the time of the latest cue, run on
/// at the clock's rate for as long as the viewer has waited since.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    /// Log time when the latest cue arrived.
    pub at: f64,
    /// Log milliseconds per viewer millisecond.
    pub rate: f64,
    /// Whether the clock runs. A saved game shown at rest stops it at its
    /// last cue.
    pub running: bool,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            at: 0.0,
            rate: 1.0,
            running: false,
        }
    }
}

impl Clock {
    /// Log time `elapsed` viewer milliseconds after the latest cue arrived.
    pub fn now(&self, elapsed: f64) -> f64 {
        if self.running {
            self.at + elapsed.max(0.0) * self.rate
        } else {
            self.at
        }
    }
}

/// A countdown: a phase's, or a tentative decision's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    /// When it started, in log time.
    pub began: Millis,
    /// How long it runs.
    pub limit_ms: u64,
}

impl Timer {
    /// Milliseconds left at log time `now`, never below zero.
    pub fn left(&self, now: f64) -> f64 {
        (self.began as f64 + self.limit_ms as f64 - now).clamp(0.0, self.limit_ms as f64)
    }

    /// Whole seconds left at log time `now`, rounded up.
    pub fn seconds(&self, now: f64) -> u64 {
        (self.left(now) / 1_000.0).ceil() as u64
    }

    /// The share of the countdown left at log time `now`, from 1 to 0.
    pub fn share(&self, now: f64) -> f64 {
        if self.limit_ms == 0 {
            0.0
        } else {
            self.left(now) / self.limit_ms as f64
        }
    }
}

/// Everything the screen shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// The game, once cast.
    pub game: Option<GameName>,
    /// The latest chapter heading.
    pub heading: Option<String>,
    /// The chat, oldest first.
    pub posts: Vec<Post>,
    /// Every seat, in seat order.
    pub seats: Vec<Seat>,
    /// Where each name's bubbles sit and their colors.
    pub names: Names,
    /// How the game ended, once it has.
    pub outcome: Option<Outcome>,
    /// The board as it stands.
    pub panel: Option<Panel>,
    /// The choices flashing now, each with the cue that landed it.
    pub landed: Vec<(usize, Landed)>,
    /// The dots of every call out longer than [`THINKING_DELAY`], in the
    /// order they showed.
    pub thinkers: Vec<Thinker>,
    /// Every call out whose dots have not shown, with when it went out.
    thinking: Vec<(PlayerId, Millis)>,
    /// The log-time clock.
    pub clock: Clock,
    /// How many cues have been applied.
    applied: usize,
    /// How many times the screen has been reset.
    era: u64,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            game: None,
            heading: None,
            posts: Vec::new(),
            seats: Vec::new(),
            names: Names::none(),
            outcome: None,
            panel: None,
            landed: Vec::new(),
            thinkers: Vec::new(),
            thinking: Vec::new(),
            clock: Clock::default(),
            applied: 0,
            era: 0,
        }
    }
}

impl Screen {
    /// Show `cue`. With `flash`, every choice a new board makes lands, the
    /// cue's number comes back for [`Screen::expire`] to end the flash,
    /// and the clock runs on from the cue; without it, as for a saved game
    /// shown at once, nothing lands and the clock stops at the cue. A cue
    /// earlier than the clock leaves it where it is. Every call out longer
    /// than [`THINKING_DELAY`] by the cue's time shows its dots.
    pub fn apply(&mut self, cue: &Cue, flash: bool) -> Option<usize> {
        self.reveal(cue.at);
        self.applied += 1;
        let number = self.applied;
        self.clock.at = self.clock.at.max(cue.at as f64);
        self.clock.running = flash;
        match &cue.beat {
            Beat::Cast { game, seats } => {
                self.game = Some(*game);
                self.seats = seats.clone();
                self.names = names(seats);
            }
            Beat::Heading(text) => {
                self.heading = Some(text.clone());
                self.post(NARRATOR.to_string(), Body::Heading(text.clone()));
            }
            Beat::Told(lines) => self.post(NARRATOR.to_string(), Body::Text(lines.join("  \n"))),
            Beat::Said { who, text } => self.post(self.label(who), Body::Text(text.clone())),
            Beat::Late { who, text } => self.post(self.label(who), Body::Late(text.clone())),
            Beat::Thinking { who } => self.thinking.push((who.clone(), cue.at)),
            Beat::Thought { who } => {
                self.thinking.retain(|(thinker, _)| thinker != who);
                self.thinkers.retain(|thinker| thinker.who != *who);
            }
            Beat::Clock { rate } => self.clock.rate = *rate,
            Beat::Panel(panel) => {
                let landed = match (flash, &self.panel, panel) {
                    (true, before, Panel::Avalon(after)) => {
                        let before = match before {
                            Some(Panel::Avalon(before)) => Some(before),
                            _ => None,
                        };
                        landings(before, after)
                    }
                    (true, before, Panel::Werewolf(after)) => {
                        let before = match before {
                            Some(Panel::Werewolf(before)) => Some(before),
                            _ => None,
                        };
                        werewolf_landings(before, after)
                    }
                    _ => vec![],
                };
                self.panel = Some(panel.clone());
                if !landed.is_empty() {
                    self.landed
                        .extend(landed.into_iter().map(|landed| (number, landed)));
                    return Some(number);
                }
            }
            Beat::Over { outcome, .. } => {
                self.outcome = Some(outcome.clone());
                self.thinking.clear();
                self.thinkers.clear();
            }
        }
        None
    }

    /// Show the dots of every call out longer than [`THINKING_DELAY`] at
    /// log time `now`.
    pub fn reveal(&mut self, now: Millis) {
        let (due, waiting) = std::mem::take(&mut self.thinking)
            .into_iter()
            .partition(|(_, at)| at + THINKING_DELAY <= now);
        self.thinking = waiting;
        for (who, _) in due {
            let thinker = Thinker {
                name: self.label(&who),
                who,
            };
            self.thinkers.push(thinker);
        }
    }

    /// Clear the screen for a new game, in a new era.
    pub fn reset(&mut self) {
        *self = Screen {
            era: self.era + 1,
            ..Screen::default()
        };
    }

    /// Which era the screen is in. Each [`Screen::reset`] starts another.
    pub fn era(&self) -> u64 {
        self.era
    }

    /// Do `act` to the screen if it is still in `era`, and nothing once it
    /// has been reset: what a timer set for one game does, kept from the
    /// next.
    pub fn within(&mut self, era: u64, act: impl FnOnce(&mut Screen)) {
        if self.era == era {
            act(self);
        }
    }

    /// End the flash of what the cue numbered `number` landed.
    pub fn expire(&mut self, number: usize) {
        self.landed.retain(|(landed_by, _)| *landed_by != number);
    }

    /// Run the clock on by `elapsed` viewer milliseconds, as the viewer
    /// does just before it applies the next cue.
    pub fn advance(&mut self, elapsed: f64) {
        self.clock.at = self.clock.now(elapsed);
    }

    /// What the Avalon board draws, once there is one.
    pub fn board(&self) -> Option<Board> {
        match &self.panel {
            Some(Panel::Avalon(panel)) => Some(self.drawn(panel.clone())),
            _ => None,
        }
    }

    /// What the Werewolf board draws, once there is one.
    pub fn werewolf(&self) -> Option<Board<WerewolfPanel>> {
        match &self.panel {
            Some(Panel::Werewolf(panel)) => Some(self.drawn(panel.clone())),
            _ => None,
        }
    }

    /// `panel` with the seats and the choices flashing on it.
    fn drawn<P>(&self, panel: P) -> Board<P> {
        Board {
            panel,
            seats: self.seats.clone(),
            landed: self
                .landed
                .iter()
                .map(|(_, landed)| landed.clone())
                .collect(),
        }
    }

    /// The phase's countdown, from the latest board of either game.
    pub fn phase_timer(&self) -> Option<Timer> {
        let (began, limit_ms) = match self.panel.as_ref()? {
            Panel::Avalon(panel) => (panel.began, panel.limit_ms),
            Panel::Werewolf(panel) => (panel.began, panel.limit_ms),
        };
        Some(Timer { began, limit_ms })
    }

    /// The root element's `data-theme`: `dark` by night and `light` by day
    /// in a Werewolf game underway, or in a saved one shown at rest, and
    /// otherwise none, so the page follows the OS.
    pub fn theme(&self) -> Option<&'static str> {
        let underway = self.outcome.is_none() || !self.clock.running;
        match &self.panel {
            Some(Panel::Werewolf(panel)) if underway => Some(match panel.phase {
                WerewolfPhase::Night => "dark",
                WerewolfPhase::Day => "light",
            }),
            _ => None,
        }
    }

    /// `player`'s label, or the name alone before the cast.
    fn label(&self, player: &str) -> String {
        self.seats
            .iter()
            .find(|seat| seat.name == player)
            .map_or_else(|| player.to_string(), |seat| seat.label.clone())
    }

    /// Post `body` from `name`.
    fn post(&mut self, name: String, body: Body) {
        let id = format!("m{}", self.posts.len() + 1);
        self.posts.push(Post { id, name, body });
    }
}

/// What a board draws: the board, the seats, and the choices flashing on
/// it.
#[derive(Debug, Clone, PartialEq)]
pub struct Board<P = AvalonPanel> {
    /// The board as it stands.
    pub panel: P,
    /// Every seat, in seat order.
    pub seats: Vec<Seat>,
    /// The choices flashing now.
    pub landed: Vec<Landed>,
}

impl<P> Board<P> {
    /// Whether `choice` is flashing.
    pub fn is_landed(&self, choice: &Landed) -> bool {
        self.landed.contains(choice)
    }

    /// The seat of `player`.
    pub fn seat(&self, player: &str) -> Option<&Seat> {
        self.seats.iter().find(|seat| seat.name == player)
    }
}

/// When the dots of the call `cue` sends out are due, in log time, if it
/// sends one out.
pub fn dots_due(cue: &Cue) -> Option<Millis> {
    matches!(cue.beat, Beat::Thinking { .. }).then_some(cue.at + THINKING_DELAY)
}

impl Board<WerewolfPanel> {
    /// The day's votes counted per target, most first, a tie in seat
    /// order.
    pub fn tally(&self) -> Vec<(PlayerId, usize)> {
        let mut tally: Vec<(PlayerId, usize)> = self
            .seats
            .iter()
            .map(|seat| {
                let votes = self.panel.votes.iter();
                let count = votes
                    .filter(|(_, vote)| vote.as_deref() == Some(seat.name.as_str()))
                    .count();
                (seat.name.clone(), count)
            })
            .filter(|(_, count)| *count > 0)
            .collect();
        tally.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        tally
    }
}

/// A day's vote row: `Player 1 → Player 6`, or `Player 1 · undecided`.
pub fn vote(player: &str, target: Option<&str>) -> String {
    match target {
        Some(target) => format!("{player} → {target}"),
        None => format!("{player} · undecided"),
    }
}

/// A night role's row: `Player 4 · kill → Player 3`, or
/// `Player 4 · choosing…` while it chooses.
pub fn deed(player: &str, deed: &NightDeed) -> String {
    match deed {
        NightDeed::Pending => format!("{player} · choosing…"),
        NightDeed::Kill(target) => format!("{player} · kill → {target}"),
        NightDeed::Protect(target) => format!("{player} · protect → {target}"),
        NightDeed::Ask(target) => format!("{player} · ask → {target}"),
    }
}

/// The tentative decision: `Player 4 out in 14s`, or `Nobody out in 14s`.
pub fn ratify(target: Option<&str>, seconds: u64) -> String {
    format!("{} out in {seconds}s", target.unwrap_or("Nobody"))
}

/// How a player died: `killed night 2`, or `voted out day 1`.
pub fn death(death: &Death) -> String {
    match death.phase {
        WerewolfPhase::Night => format!("killed night {}", death.round),
        WerewolfPhase::Day => format!("voted out day {}", death.round),
    }
}

/// What a seer learned: `Player 6 is a werewolf · night 1`.
pub fn learned(learned: &Learned) -> String {
    format!(
        "{} is a {} · night {}",
        learned.player, learned.role, learned.night
    )
}

/// How a night turned out: `2 · wolves chose Player 2, who died`, or
/// `1 · wolves chose Player 3, protected by Player 1`.
pub fn night(night: &NightOutcome) -> String {
    let what = match (&night.victim, &night.died) {
        (None, _) => "wolves chose nobody".to_string(),
        (Some(victim), Some(_)) => format!("wolves chose {victim}, who died"),
        (Some(victim), None) => format!(
            "wolves chose {victim}, protected by {}",
            night.protected_by.join(" and ")
        ),
    };
    format!("{} · {what}", night.night)
}

/// `player`'s choice in `entries`, if made.
pub fn entry<T: Copy>(entries: &[(PlayerId, Option<T>)], player: &str) -> Option<T> {
    entries
        .iter()
        .find(|(name, _)| name == player)
        .and_then(|(_, choice)| *choice)
}

/// The names table for `seats`: evil on the left and good on the right,
/// each in its role's colors, and the narrator centered and muted.
fn names(seats: &[Seat]) -> Names {
    let narrator = Look::at(Position::Center)
        .background("transparent")
        .foreground("var(--rc-muted)");
    seats
        .iter()
        .fold(Names::none().name(NARRATOR, narrator), |names, seat| {
            let position = match seat.side {
                Side::Evil => Position::Left,
                Side::Good => Position::Right,
            };
            let role = role_key(seat.role);
            let look = Look::at(position)
                .background(format!("var(--role-{role}-bg)"))
                .foreground(format!("var(--role-{role}-fg)"));
            names.name(seat.label.clone(), look)
        })
}

/// `role` as the stylesheet's custom properties name it: `--role-seer-bg`.
pub fn role_key(role: Role) -> &'static str {
    match role {
        Role::Villager => "villager",
        Role::Seer => "seer",
        Role::Doctor => "doctor",
        Role::Werewolf => "werewolf",
        Role::Merlin => "merlin",
        Role::Percival => "percival",
        Role::Servant => "servant",
        Role::Assassin => "assassin",
        Role::Morgana => "morgana",
        Role::Mordred => "mordred",
        Role::Oberon => "oberon",
        Role::Minion => "minion",
    }
}

/// The choices `after` makes that the board `before` it did not have: a
/// newly proposed team, each new ballot, each new deed, and the
/// Assassin's target.
pub fn landings(before: Option<&AvalonPanel>, after: &AvalonPanel) -> Vec<Landed> {
    let mut landed = Vec::new();
    if after.team.is_some() && after.team != before.and_then(|before| before.team.clone()) {
        landed.push(Landed::Team);
    }
    for row in &after.proposals {
        let earlier = before.and_then(|before| {
            before
                .proposals
                .iter()
                .find(|old| old.quest == row.quest && old.proposal == row.proposal)
        });
        for (player, ballot) in &row.ballots {
            let was = earlier.and_then(|old| entry(&old.ballots, player));
            if ballot.is_some() && *ballot != was {
                landed.push(Landed::Ballot {
                    quest: row.quest,
                    proposal: row.proposal,
                    player: player.clone(),
                });
            }
        }
    }
    for (player, deed) in &after.deeds {
        let was = before.and_then(|before| entry(&before.deeds, player));
        if deed.is_some() && *deed != was {
            landed.push(Landed::Deed(player.clone()));
        }
    }
    let target = |panel: &AvalonPanel| {
        panel
            .assassination
            .as_ref()
            .and_then(|shot| shot.target.clone())
    };
    if target(after).is_some() && target(after) != before.and_then(target) {
        landed.push(Landed::Target);
    }
    landed
}

/// The choices `after` makes that the Werewolf board `before` it did not
/// have: each vote changed, a retraction among them, each night deed
/// changed, a new tentative decision, and each new death.
pub fn werewolf_landings(before: Option<&WerewolfPanel>, after: &WerewolfPanel) -> Vec<Landed> {
    let mut landed = Vec::new();
    for (player, target) in &after.votes {
        let was = before
            .and_then(|before| before.votes.iter().find(|(name, _)| name == player))
            .and_then(|(_, target)| target.as_ref());
        if target.as_ref() != was {
            landed.push(Landed::Vote(player.clone()));
        }
    }
    for (player, deed) in &after.night {
        let was = before
            .and_then(|before| before.night.iter().find(|(name, _)| name == player))
            .map_or(&NightDeed::Pending, |(_, deed)| deed);
        if deed != was {
            landed.push(Landed::Night(player.clone()));
        }
    }
    let target = |panel: &WerewolfPanel| panel.ratify.as_ref().map(|ratify| ratify.target.clone());
    if target(after).is_some() && target(after) != before.and_then(target) {
        landed.push(Landed::Ratify);
    }
    for seat in &after.seats {
        let was = before
            .and_then(|before| before.seats.iter().find(|old| old.name == seat.name))
            .and_then(|old| old.died);
        if seat.died.is_some() && was.is_none() {
            landed.push(Landed::Death(seat.name.clone()));
        }
    }
    landed
}

#[cfg(test)]
mod tests {
    use super::*;
    use social_deduction_stage::{
        Assassination, AvalonPhase, AvalonSeat, Deed, ProposalRow, QuestToken, Ratify, SeatState,
    };

    /// `beat` at time 0.
    fn cue(beat: Beat) -> Cue {
        Cue { at: 0, beat }
    }

    /// A seat of `role` on `side`.
    fn seat(name: &str, role: Role, side: Side) -> Seat {
        Seat {
            name: name.to_string(),
            label: format!("{name} · {role}"),
            role,
            side,
            strategy: "random".to_string(),
        }
    }

    /// The cast of a game of three.
    fn cast() -> Cue {
        cue(Beat::Cast {
            game: GameName::Avalon,
            seats: vec![
                seat("Player 1", Role::Merlin, Side::Good),
                seat("Player 2", Role::Assassin, Side::Evil),
                seat("Player 3", Role::Servant, Side::Good),
            ],
        })
    }

    /// A board of three at the first proposal, with nothing chosen.
    fn board() -> AvalonPanel {
        let players = ["Player 1", "Player 2", "Player 3"];
        AvalonPanel {
            quest: 1,
            proposal: 1,
            phase: AvalonPhase::Vote,
            began: 0,
            limit_ms: 60_000,
            leader: "Player 1".to_string(),
            team: None,
            deeds: vec![],
            quests: vec![
                QuestToken {
                    size: 2,
                    sabotages_needed: 1,
                    result: None,
                };
                5
            ],
            proposals: vec![ProposalRow {
                quest: 1,
                proposal: 1,
                leader: "Player 1".to_string(),
                team: None,
                ballots: players.map(|p| (p.to_string(), None)).to_vec(),
                approved: None,
                result: None,
            }],
            seats: players
                .map(|p| AvalonSeat {
                    name: p.to_string(),
                    sees: vec![],
                })
                .to_vec(),
            assassination: None,
        }
    }

    /// `panel` as a cue.
    fn panel(panel: &AvalonPanel) -> Cue {
        cue(Beat::Panel(Panel::Avalon(panel.clone())))
    }

    /// A short game: the cast, narration, talk, two boards and the end.
    fn game() -> Vec<Cue> {
        let mut voted = board();
        voted.team = Some(vec!["Player 1".to_string(), "Player 2".to_string()]);
        voted.proposals[0].ballots[1].1 = Some(false);
        vec![
            cast(),
            cue(Beat::Clock { rate: 1.0 }),
            panel(&board()),
            cue(Beat::Heading("Quest 1.".to_string())),
            cue(Beat::Said {
                who: "Player 2".to_string(),
                text: "Trust me.".to_string(),
            }),
            panel(&voted),
            cue(Beat::Told(vec!["The team does not go.".to_string()])),
            cue(Beat::Over {
                outcome: Outcome::Won {
                    side: Side::Evil,
                    how: "Evil wins by naming Merlin.".to_string(),
                },
                log: None,
            }),
        ]
    }

    /// The screen after `cues`, applied at once.
    fn shown(cues: &[Cue]) -> Screen {
        let mut screen = Screen::default();
        for cue in cues {
            screen.apply(cue, false);
        }
        screen
    }

    #[test]
    fn the_cast_sets_the_names_table_with_evil_left_and_good_right() {
        let screen = shown(&[cast()]);
        assert_eq!(screen.game, Some(GameName::Avalon));
        let look = |name: &str| screen.names.get(name).cloned().unwrap();
        assert_eq!(
            look("Player 1 · Merlin"),
            Look::at(Position::Right)
                .background("var(--role-merlin-bg)")
                .foreground("var(--role-merlin-fg)")
        );
        assert_eq!(look("Player 2 · Assassin").position, Position::Left);
        assert_eq!(look("Player 3 · Servant").position, Position::Right);
        assert_eq!(
            look(NARRATOR),
            Look::at(Position::Center)
                .background("transparent")
                .foreground("var(--rc-muted)")
        );
        assert!(screen.posts.is_empty());
    }

    #[test]
    fn told_lines_are_one_centered_message_of_hard_broken_lines() {
        let told = cue(Beat::Told(vec!["One.".to_string(), "Two.".to_string()]));
        let screen = shown(&[cast(), told]);
        assert_eq!(
            screen.posts,
            [Post {
                id: "m1".to_string(),
                name: NARRATOR.to_string(),
                body: Body::Text("One.  \nTwo.".to_string()),
            }]
        );
    }

    #[test]
    fn headings_and_talk_are_posted_in_order_and_the_heading_tops_the_screen() {
        let screen = shown(&game());
        let posted: Vec<(&str, &str, &Body)> = screen
            .posts
            .iter()
            .map(|post| (post.id.as_str(), post.name.as_str(), &post.body))
            .collect();
        assert_eq!(
            posted,
            [
                ("m1", NARRATOR, &Body::Heading("Quest 1.".to_string())),
                (
                    "m2",
                    "Player 2 · Assassin",
                    &Body::Text("Trust me.".to_string())
                ),
                (
                    "m3",
                    NARRATOR,
                    &Body::Text("The team does not go.".to_string())
                ),
            ]
        );
        assert_eq!(screen.heading.as_deref(), Some("Quest 1."));
    }

    #[test]
    fn talk_before_the_cast_is_from_the_speakers_name() {
        let said = cue(Beat::Said {
            who: "Player 9".to_string(),
            text: "Hi.".to_string(),
        });
        assert_eq!(shown(&[said]).posts[0].name, "Player 9");
    }

    #[test]
    fn the_end_sets_the_outcome() {
        let screen = shown(&game());
        assert_eq!(
            screen.outcome,
            Some(Outcome::Won {
                side: Side::Evil,
                how: "Evil wins by naming Merlin.".to_string()
            })
        );
    }

    #[test]
    fn a_saved_game_shown_at_once_flashes_nothing() {
        let screen = shown(&game());
        assert!(screen.landed.is_empty());
        assert!(screen.panel.is_some());
    }

    #[test]
    fn two_boards_in_a_row_flash_exactly_the_choices_that_changed() {
        let mut screen = Screen::default();
        screen.apply(&cast(), true);
        assert_eq!(screen.apply(&panel(&board()), true), None);
        let mut next = board();
        next.team = Some(vec!["Player 1".to_string(), "Player 3".to_string()]);
        next.proposals[0].ballots[2].1 = Some(true);
        let number = screen.apply(&panel(&next), true).unwrap();
        assert_eq!(
            screen.landed,
            [
                (number, Landed::Team),
                (
                    number,
                    Landed::Ballot {
                        quest: 1,
                        proposal: 1,
                        player: "Player 3".to_string()
                    }
                )
            ]
        );
        assert!(screen.board().unwrap().is_landed(&Landed::Team));
        // The same board again lands nothing more.
        assert_eq!(screen.apply(&panel(&next), true), None);
        screen.expire(number);
        assert!(screen.landed.is_empty());
    }

    #[test]
    fn a_reset_clears_the_screen_and_what_was_asked_before_it_is_ignored() {
        let mut screen = Screen::default();
        screen.apply(&cast(), true);
        screen.apply(&panel(&board()), true);
        let mut next = board();
        next.team = Some(vec!["Player 1".to_string(), "Player 3".to_string()]);
        let before = screen.era();
        let expired = screen.apply(&panel(&next), true).unwrap();
        screen.reset();
        assert_ne!(screen.era(), before);
        assert_eq!(
            Screen {
                era: 0,
                ..screen.clone()
            },
            Screen::default()
        );
        screen.apply(&cast(), true);
        screen.apply(&panel(&board()), true);
        let number = screen.apply(&panel(&next), true).unwrap();
        assert_eq!(number, expired);
        screen.within(before, |screen| screen.expire(expired));
        assert_eq!(screen.landed, [(number, Landed::Team)]);
        let now = screen.era();
        screen.within(now, |screen| screen.expire(number));
        assert!(screen.landed.is_empty());
    }

    #[test]
    fn a_deed_and_the_assassins_target_land_once_each() {
        let mut before = board();
        before.deeds = vec![
            ("Player 1".to_string(), None),
            ("Player 2".to_string(), None),
        ];
        before.assassination = Some(Assassination {
            assassin: "Player 2".to_string(),
            target: None,
            merlin: None,
        });
        let mut after = before.clone();
        after.deeds[1].1 = Some(Deed::Sabotage);
        after.assassination.as_mut().unwrap().target = Some("Player 1".to_string());
        assert_eq!(
            landings(Some(&before), &after),
            [Landed::Deed("Player 2".to_string()), Landed::Target]
        );
        assert!(landings(Some(&after), &after).is_empty());
        assert_eq!(landings(None, &after).len(), 2);
    }

    #[test]
    fn replaying_a_game_from_the_start_ends_on_the_same_screen() {
        let cues = game();
        let mut replayed = Screen::default();
        let mut flashes = Vec::new();
        for cue in &cues {
            flashes.extend(replayed.apply(cue, true));
        }
        assert!(!flashes.is_empty());
        for number in flashes {
            replayed.expire(number);
        }
        assert!(replayed.clock.running);
        replayed.clock.running = false;
        assert_eq!(replayed, shown(&cues));
    }

    /// `beat` at log time `at`.
    fn cue_at(at: Millis, beat: Beat) -> Cue {
        Cue { at, beat }
    }

    /// Player 2's call going out at `at`.
    fn thinking(at: Millis) -> Cue {
        cue_at(
            at,
            Beat::Thinking {
                who: "Player 2".to_string(),
            },
        )
    }

    /// Player 2's call coming back at `at`.
    fn thought(at: Millis) -> Cue {
        cue_at(
            at,
            Beat::Thought {
                who: "Player 2".to_string(),
            },
        )
    }

    /// The thinker Player 2's dots are.
    fn thinker() -> Thinker {
        Thinker {
            who: "Player 2".to_string(),
            name: "Player 2 · Assassin".to_string(),
        }
    }

    #[test]
    fn a_call_that_comes_back_within_its_delay_shows_no_dots() {
        let mut screen = shown(&[cast(), thinking(1_000)]);
        assert_eq!(dots_due(&thinking(1_000)), Some(1_300));
        screen.apply(&thought(1_299), true);
        screen.reveal(1_300);
        assert!(screen.thinkers.is_empty());
    }

    #[test]
    fn a_call_held_past_its_delay_shows_dots_until_it_comes_back() {
        let mut screen = shown(&[cast(), thinking(1_000)]);
        screen.reveal(1_299);
        assert!(screen.thinkers.is_empty());
        screen.reveal(1_300);
        assert_eq!(screen.thinkers, [thinker()]);
        screen.reveal(1_400);
        assert_eq!(screen.thinkers, [thinker()]);
        screen.apply(&thought(2_000), true);
        assert!(screen.thinkers.is_empty());
    }

    #[test]
    fn a_later_cue_shows_the_dots_of_every_call_held_past_its_delay() {
        let said = cue_at(
            1_500,
            Beat::Said {
                who: "Player 1".to_string(),
                text: "Hm.".to_string(),
            },
        );
        let screen = shown(&[cast(), thinking(1_000), said]);
        assert_eq!(screen.thinkers, [thinker()]);
        assert_eq!(screen.posts.len(), 1);
    }

    #[test]
    fn the_end_clears_every_thinker() {
        let mut cues = vec![cast(), thinking(0), cue_at(500, Beat::Told(vec![]))];
        cues.extend(game().into_iter().skip(1));
        let mut screen = shown(&cues[..3]);
        assert_eq!(screen.thinkers.len(), 1);
        screen = shown(&cues);
        assert!(screen.thinkers.is_empty());
        screen.reveal(10_000);
        assert!(screen.thinkers.is_empty());
    }

    #[test]
    fn only_a_call_going_out_waits_to_show_its_dots() {
        assert_eq!(dots_due(&cast()), None);
    }

    #[test]
    fn late_speech_is_posted_from_its_speaker_as_late() {
        let late = cue(Beat::Late {
            who: "Player 2".to_string(),
            text: "Wait!".to_string(),
        });
        let screen = shown(&[cast(), late]);
        assert_eq!(
            screen.posts,
            [Post {
                id: "m1".to_string(),
                name: "Player 2 · Assassin".to_string(),
                body: Body::Late("Wait!".to_string()),
            }]
        );
    }

    #[test]
    fn the_clock_runs_at_its_rate_from_the_latest_cue_and_never_runs_backward() {
        let mut screen = Screen::default();
        screen.apply(&cue_at(1_000, Beat::Clock { rate: 2.0 }), true);
        assert_eq!(screen.clock.now(0.0), 1_000.0);
        assert_eq!(screen.clock.now(250.0), 1_500.0);
        assert_eq!(screen.clock.now(-5.0), 1_000.0);
        // Half a second on, a cue stamped earlier than the clock leaves
        // it where it is.
        screen.advance(500.0);
        screen.apply(&cue_at(1_200, Beat::Heading("Day 1.".to_string())), true);
        assert_eq!(screen.clock.now(0.0), 2_000.0);
        // A later one moves it on.
        screen.advance(0.0);
        screen.apply(&cue_at(5_000, Beat::Heading("Night 2.".to_string())), true);
        assert_eq!(screen.clock.now(100.0), 5_200.0);
    }

    #[test]
    fn a_saved_game_at_rest_stops_the_clock_at_its_last_cue() {
        let cues = [
            cue_at(1_000, Beat::Clock { rate: 1.0 }),
            cue_at(9_000, Beat::Heading("Day 1.".to_string())),
        ];
        let mut screen = shown(&cues);
        assert_eq!(screen.clock.now(60_000.0), 9_000.0);
        screen.advance(60_000.0);
        assert_eq!(screen.clock.at, 9_000.0);
    }

    #[test]
    fn a_timer_counts_down_its_limit_from_its_start() {
        let timer = Timer {
            began: 1_000,
            limit_ms: 20_000,
        };
        assert_eq!(timer.left(0.0), 20_000.0);
        assert_eq!(timer.left(6_000.0), 15_000.0);
        assert_eq!(timer.seconds(6_500.0), 15);
        assert_eq!(timer.share(11_000.0), 0.5);
        assert_eq!(timer.left(30_000.0), 0.0);
        assert_eq!(timer.seconds(30_000.0), 0);
        let none = Timer {
            began: 0,
            limit_ms: 0,
        };
        assert_eq!(none.share(0.0), 0.0);
    }

    #[test]
    fn the_phase_timer_is_the_latest_boards_of_either_game() {
        assert_eq!(Screen::default().phase_timer(), None);
        let avalon = shown(&[cast(), panel(&board())]);
        assert_eq!(
            avalon.phase_timer(),
            Some(Timer {
                began: 0,
                limit_ms: 60_000
            })
        );
        let werewolf = shown(&[werewolf_cast(), tonight()]);
        assert_eq!(
            werewolf.phase_timer(),
            Some(Timer {
                began: 1_000,
                limit_ms: 30_000
            })
        );
    }

    /// The cast of a Werewolf game of four: a werewolf, a doctor, a seer
    /// and a villager.
    fn werewolf_cast() -> Cue {
        cue(Beat::Cast {
            game: GameName::Werewolf,
            seats: vec![
                seat("Player 1", Role::Werewolf, Side::Evil),
                seat("Player 2", Role::Doctor, Side::Good),
                seat("Player 3", Role::Seer, Side::Good),
                seat("Player 4", Role::Villager, Side::Good),
            ],
        })
    }

    /// The four's first night, with nobody yet chosen.
    fn first_night() -> WerewolfPanel {
        let players = ["Player 1", "Player 2", "Player 3", "Player 4"];
        WerewolfPanel {
            round: 1,
            phase: WerewolfPhase::Night,
            began: 1_000,
            limit_ms: 30_000,
            ratify: None,
            seats: players
                .map(|name| SeatState {
                    name: name.to_string(),
                    died: None,
                })
                .to_vec(),
            votes: vec![],
            night: players[..3]
                .iter()
                .map(|name| (name.to_string(), NightDeed::Pending))
                .collect(),
            seers: vec![("Player 3".to_string(), vec![])],
            nights: vec![],
        }
    }

    /// The four's first day, everyone alive and undecided.
    fn first_day() -> WerewolfPanel {
        let mut day = first_night();
        day.phase = WerewolfPhase::Day;
        day.night = vec![];
        day.votes = day
            .seats
            .iter()
            .map(|seat| (seat.name.clone(), None))
            .collect();
        day
    }

    /// `panel` as a cue.
    fn werewolf(panel: &WerewolfPanel) -> Cue {
        cue(Beat::Panel(Panel::Werewolf(panel.clone())))
    }

    /// The first night as a cue.
    fn tonight() -> Cue {
        werewolf(&first_night())
    }

    #[test]
    fn night_is_dark_and_day_light_and_otherwise_the_page_follows_the_os() {
        assert_eq!(Screen::default().theme(), None);
        assert_eq!(shown(&[werewolf_cast()]).theme(), None);
        assert_eq!(shown(&[werewolf_cast(), tonight()]).theme(), Some("dark"));
        let day = werewolf(&first_day());
        assert_eq!(
            shown(&[werewolf_cast(), tonight(), day.clone()]).theme(),
            Some("light")
        );
        let over = cue(Beat::Over {
            outcome: Outcome::Stopped,
            log: None,
        });
        let ended = [werewolf_cast(), day, over];
        // A saved game at rest takes its final phase's theme, and one
        // played through follows the OS once it is over.
        assert_eq!(shown(&ended).theme(), Some("light"));
        let mut played = Screen::default();
        for cue in &ended {
            played.apply(cue, true);
        }
        assert_eq!(played.theme(), None);
        assert_eq!(shown(&game()).theme(), None);
    }

    /// The choices that land as `after` follows `before` on a screen.
    fn landed_between(before: &WerewolfPanel, after: &WerewolfPanel) -> Vec<Landed> {
        let mut screen = Screen::default();
        screen.apply(&werewolf_cast(), false);
        screen.apply(&werewolf(before), false);
        let number = screen.apply(&werewolf(after), true);
        let landed: Vec<Landed> = screen
            .landed
            .iter()
            .map(|(_, landed)| landed.clone())
            .collect();
        assert_eq!(number.is_some(), !landed.is_empty());
        if let Some(number) = number {
            assert!(screen.landed.iter().all(|(by, _)| *by == number));
        }
        landed
    }

    #[test]
    fn a_vote_and_a_retraction_each_land_exactly_their_row() {
        let mut voted = first_day();
        voted.votes[1].1 = Some("Player 1".to_string());
        assert_eq!(
            landed_between(&first_day(), &voted),
            [Landed::Vote("Player 2".to_string())]
        );
        let mut changed = voted.clone();
        changed.votes[1].1 = Some("Player 4".to_string());
        assert_eq!(
            landed_between(&voted, &changed),
            [Landed::Vote("Player 2".to_string())]
        );
        assert_eq!(
            landed_between(&voted, &first_day()),
            [Landed::Vote("Player 2".to_string())]
        );
        assert!(landed_between(&voted, &voted).is_empty());
        // A new day of undecided votes lands nothing.
        assert!(landed_between(&first_night(), &first_day()).is_empty());
    }

    #[test]
    fn a_night_deed_lands_its_row() {
        let mut chosen = first_night();
        chosen.night[1].1 = NightDeed::Protect("Player 4".to_string());
        assert_eq!(
            landed_between(&first_night(), &chosen),
            [Landed::Night("Player 2".to_string())]
        );
        assert!(landed_between(&first_day(), &first_night()).is_empty());
    }

    #[test]
    fn a_death_lands_its_seat() {
        let mut dead = first_day();
        dead.seats[3].died = Some(Death {
            round: 1,
            phase: WerewolfPhase::Night,
        });
        assert_eq!(
            landed_between(&first_night(), &dead),
            [Landed::Death("Player 4".to_string())]
        );
        assert!(landed_between(&dead, &dead).is_empty());
    }

    #[test]
    fn a_tentative_decision_lands_when_its_target_changes() {
        let ratify = |target: Option<&str>, since| {
            let mut day = first_day();
            day.ratify = Some(Ratify {
                target: target.map(str::to_string),
                since,
                limit_ms: 20_000,
            });
            day
        };
        let first = ratify(Some("Player 1"), 3_000);
        assert_eq!(landed_between(&first_day(), &first), [Landed::Ratify]);
        assert!(landed_between(&first, &ratify(Some("Player 1"), 4_000)).is_empty());
        assert_eq!(
            landed_between(&first, &ratify(None, 5_000)),
            [Landed::Ratify]
        );
        assert!(landed_between(&first, &first_day()).is_empty());
    }

    #[test]
    fn a_werewolf_board_after_an_avalon_one_lands_as_if_first() {
        let mut screen = Screen::default();
        screen.apply(&panel(&board()), true);
        let mut voted = first_day();
        voted.votes[0].1 = Some("Player 2".to_string());
        screen.apply(&werewolf(&voted), true);
        assert_eq!(
            screen.werewolf().unwrap().landed,
            [Landed::Vote("Player 1".to_string())]
        );
        assert_eq!(screen.board(), None);
        assert_eq!(shown(&[panel(&board())]).werewolf(), None);
    }

    #[test]
    fn the_votes_are_counted_most_first_and_a_tie_in_seat_order() {
        let mut day = first_day();
        day.votes[0].1 = Some("Player 4".to_string());
        day.votes[1].1 = Some("Player 3".to_string());
        day.votes[2].1 = Some("Player 4".to_string());
        day.votes[3].1 = Some("Player 2".to_string());
        let screen = shown(&[werewolf_cast(), werewolf(&day)]);
        let board = screen.werewolf().unwrap();
        assert_eq!(
            board.tally(),
            [
                ("Player 4".to_string(), 2),
                ("Player 2".to_string(), 1),
                ("Player 3".to_string(), 1)
            ]
        );
    }

    #[test]
    fn the_clock_sets_the_rate() {
        assert_eq!(Screen::default().clock.rate, 1.0);
        let screen = shown(&[cue(Beat::Clock { rate: 4.0 })]);
        assert_eq!(screen.clock.rate, 4.0);
    }

    #[test]
    fn late_bubbles_are_styled() {
        let css = include_str!("../style.css");
        assert!(css.contains(".sd-late"));
    }

    #[test]
    fn the_board_writes_its_rows_as_the_console_would_say_them() {
        assert_eq!(vote("Player 1", Some("Player 6")), "Player 1 → Player 6");
        assert_eq!(vote("Player 1", None), "Player 1 · undecided");
        let target = || "Player 3".to_string();
        assert_eq!(
            deed("Player 4", &NightDeed::Pending),
            "Player 4 · choosing…"
        );
        assert_eq!(
            deed("Player 4", &NightDeed::Kill(target())),
            "Player 4 · kill → Player 3"
        );
        assert_eq!(
            deed("Player 1", &NightDeed::Protect(target())),
            "Player 1 · protect → Player 3"
        );
        assert_eq!(
            deed("Player 5", &NightDeed::Ask(target())),
            "Player 5 · ask → Player 3"
        );
        assert_eq!(ratify(Some("Player 4"), 14), "Player 4 out in 14s");
        assert_eq!(ratify(None, 3), "Nobody out in 3s");
        let died = |round, phase| death(&Death { round, phase });
        assert_eq!(died(2, WerewolfPhase::Night), "killed night 2");
        assert_eq!(died(1, WerewolfPhase::Day), "voted out day 1");
        let outcome = |victim: Option<&str>, protected_by: &[&str], died: Option<&str>| {
            super::night(&NightOutcome {
                night: 2,
                victim: victim.map(str::to_string),
                protected_by: protected_by.iter().map(|name| name.to_string()).collect(),
                died: died.map(str::to_string),
            })
        };
        assert_eq!(
            outcome(Some("Player 2"), &[], Some("Player 2")),
            "2 · wolves chose Player 2, who died"
        );
        assert_eq!(
            outcome(Some("Player 3"), &["Player 1", "Player 5"], None),
            "2 · wolves chose Player 3, protected by Player 1 and Player 5"
        );
        assert_eq!(outcome(None, &[], None), "2 · wolves chose nobody");
        let learned = Learned {
            player: "Player 6".to_string(),
            role: Role::Werewolf,
            night: 1,
        };
        assert_eq!(super::learned(&learned), "Player 6 is a werewolf · night 1");
    }

    #[test]
    fn every_role_has_its_own_colors() {
        let roles = [
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
        let keys: std::collections::HashSet<&str> = roles.map(role_key).into();
        assert_eq!(keys.len(), roles.len());
        let css = include_str!("../style.css");
        for key in keys {
            assert!(css.contains(&format!("--role-{key}-bg:")), "{key}");
            assert!(css.contains(&format!("--role-{key}-fg:")), "{key}");
        }
    }
}
