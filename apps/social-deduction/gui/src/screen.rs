//! What the screen shows, as a pure function of the cues so far.
//!
//! [`Screen::apply`] folds one cue at a time into the chat, the names
//! table, the header, the clock and the board. The view draws a `Screen`
//! and holds no state of its own, so the whole viewer is tested here,
//! natively. A [`Timeline`] holds the screen at each bubble of a game
//! already folded, for review.

use std::sync::Arc;

use free_agent_gui::clock::{Clock, Timer};
use leptos_rich_chat::{Look, Names, Position};
use social_deduction_gui_types::{
    AvalonPanel, Beat, Cue, Death, GameName, Learned, Millis, NightDeed, NightOutcome, Outcome,
    Panel, PlayerId, ProposalRow, QuestToken, Seat, Side, WerewolfPanel, WerewolfPhase,
};

/// The name narration is posted under, centered between the sides.
pub const NARRATOR: &str = "Narrator";

/// What the header shows in place of the clock while a Werewolf phase
/// closes.
pub const CLOSING: &str = "Closing…";

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
    /// When the game ended, in log time, once it has.
    ended: Option<Millis>,
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
    /// The bubble under review, and `None` in live play.
    pub cursor: Option<usize>,
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
            ended: None,
            panel: None,
            landed: Vec::new(),
            thinkers: Vec::new(),
            thinking: Vec::new(),
            clock: Clock::default(),
            cursor: None,
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
                let (title, rest) = chapter(text);
                self.heading = Some(title.clone());
                self.post(NARRATOR.to_string(), Body::Heading(title));
                if let Some(rest) = rest {
                    self.post(NARRATOR.to_string(), Body::Text(rest));
                }
            }
            Beat::Told(lines) => self.post(NARRATOR.to_string(), Body::Text(lines.join("  \n"))),
            Beat::Said { who, text } => self.post(self.label(who), Body::Text(text.clone())),
            Beat::Thinking { who } => self.thinking.push((who.clone(), cue.at)),
            Beat::Thought { who } => {
                self.thinking.retain(|(thinker, _)| thinker != who);
                self.thinkers.retain(|thinker| thinker.who != *who);
            }
            Beat::Clock { rate } => self.clock.rate = *rate,
            Beat::Panel(panel) => {
                let landed = if flash {
                    panel_landings(self.panel.as_ref(), panel)
                } else {
                    vec![]
                };
                self.panel = Some(panel.clone());
                if !landed.is_empty() {
                    self.landed
                        .extend(landed.into_iter().map(|landed| (number, landed)));
                    return Some(number);
                }
            }
            Beat::Over { outcome } => {
                self.outcome = Some(outcome.clone());
                self.ended = Some(cue.at);
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

    /// Show `shown`, a screen under review, in place of this one and in its
    /// era, with `landed` flashing. As with [`Screen::apply`], the number
    /// that ends the flash comes back when something lands.
    pub fn review(&mut self, shown: Screen, landed: Vec<Landed>) -> Option<usize> {
        let number = self.applied + 1;
        *self = Screen {
            landed: landed.into_iter().map(|landed| (number, landed)).collect(),
            applied: number,
            era: self.era,
            ..shown
        };
        (!self.landed.is_empty()).then_some(number)
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
            ended: self.ended,
            seats: self.seats.clone(),
            landed: self
                .landed
                .iter()
                .map(|(_, landed)| landed.clone())
                .collect(),
        }
    }

    /// The phase's countdown, from the latest board of either game, while
    /// the game goes on and the phase is not closing.
    pub fn phase_timer(&self) -> Option<Timer> {
        if self.outcome.is_some() {
            return None;
        }
        let (began, limit_ms) = match self.panel.as_ref()? {
            Panel::Avalon(panel) => (panel.began, panel.limit_ms),
            Panel::Werewolf(panel) if panel.closing.is_none() => (panel.began, panel.limit_ms),
            Panel::Werewolf(_) => return None,
        };
        Some(Timer {
            began,
            limit_ms,
            stopped: None,
        })
    }

    /// Whether the Werewolf phase underway is closing, when the header
    /// shows [`CLOSING`] in place of its clock.
    pub fn closing(&self) -> bool {
        self.outcome.is_none()
            && matches!(&self.panel, Some(Panel::Werewolf(panel)) if panel.closing.is_some())
    }

    /// The root element's `data-theme` with the app's appearance held to
    /// `appearance`: `dark` by night and `light` by day in a Werewolf game
    /// underway, or in a saved one shown at rest, and otherwise the
    /// appearance, or none, so the page follows the OS.
    pub fn theme(&self, appearance: Option<&'static str>) -> Option<&'static str> {
        let underway = self.outcome.is_none() || !self.clock.running;
        match &self.panel {
            Some(Panel::Werewolf(panel)) if underway => Some(match panel.phase {
                WerewolfPhase::Night => "dark",
                WerewolfPhase::Day => "light",
            }),
            _ => appearance,
        }
    }

    /// The seat of the player whose label is `label`, or `None` for a
    /// name that is not a player's.
    pub fn seat(&self, label: &str) -> Option<Seat> {
        self.seats.iter().find(|seat| seat.label == label).cloned()
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

/// The key the chat knows the post `id` by in `era`, `3-m12`, so that a
/// post of one game is never taken for the post of another with its ID.
pub fn key(era: u64, id: &str) -> String {
    format!("{era}-{id}")
}

/// The bubble whose chat key is `key`: bubble `k` is posted as `m{k+1}`.
pub fn bubble(key: &str) -> Option<usize> {
    let (_, id) = key.split_once('-')?;
    id.strip_prefix('m')?.parse::<usize>().ok()?.checked_sub(1)
}

/// The screen at every bubble of a game: the game's last screen, and one
/// [`Moment`] per post. Headings are posts, so they are stops too.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    /// The screen after every cue.
    last: Screen,
    /// The moment of each post, in post order.
    moments: Vec<Moment>,
}

/// What the screen shows while a bubble is the latest: everything that
/// happened from its cue up to the cue that posts the next bubble, or to
/// the end of the game for the last one. A bubble telling a death so
/// shows the board after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Moment {
    /// The board. Moments under one board share it.
    pub panel: Option<Arc<Panel>>,
    /// The latest chapter heading.
    pub heading: Option<String>,
    /// The log time.
    pub at: Millis,
    /// Whether the game is over.
    pub over: bool,
}

impl Timeline {
    /// The timeline of the game `cues` tell, folded at once.
    pub fn new(cues: &[Cue]) -> Self {
        let mut screen = Screen::default();
        let mut moments = Vec::new();
        let mut board: Option<Arc<Panel>> = None;
        let mut moment = |screen: &Screen| {
            if board.as_deref() != screen.panel.as_ref() {
                board = screen.panel.clone().map(Arc::new);
            }
            Moment {
                panel: board.clone(),
                heading: screen.heading.clone(),
                at: screen.clock.at as Millis,
                over: screen.outcome.is_some(),
            }
        };
        for cue in cues {
            let before = screen.posts.len();
            let now = (moments.len() < before).then(|| moment(&screen));
            screen.apply(cue, false);
            if let Some(now) = now.filter(|_| screen.posts.len() > before) {
                moments.resize(before, now);
            }
        }
        let now = moment(&screen);
        moments.resize(screen.posts.len(), now);
        Self {
            last: screen,
            moments,
        }
    }

    /// How many bubbles there are.
    pub fn len(&self) -> usize {
        self.moments.len()
    }

    /// Whether there are no bubbles.
    pub fn is_empty(&self) -> bool {
        self.moments.is_empty()
    }

    /// Each bubble's moment, in post order.
    pub fn moments(&self) -> &[Moment] {
        &self.moments
    }

    /// The bubbles that are chapter headings, in order.
    pub fn chapters(&self) -> Vec<usize> {
        let posts = self.last.posts.iter().enumerate();
        posts
            .filter(|(_, post)| matches!(post.body, Body::Heading(_)))
            .map(|(k, _)| k)
            .collect()
    }

    /// How the game ended, once it has.
    pub fn outcome(&self) -> Option<&Outcome> {
        self.last.outcome.as_ref()
    }

    /// The game, once cast.
    pub fn game(&self) -> Option<GameName> {
        self.last.game
    }

    /// The screen with the cursor on bubble `k`: every post, and the
    /// board, heading and outcome of its moment, the clock stopped at its
    /// time and nobody thinking.
    ///
    /// # Panics
    ///
    /// When there is no bubble `k`.
    pub fn screen(&self, k: usize) -> Screen {
        let moment = &self.moments[k];
        Screen {
            heading: moment.heading.clone(),
            panel: moment.panel.as_deref().cloned(),
            outcome: self.last.outcome.clone().filter(|_| moment.over),
            ended: self.last.ended.filter(|_| moment.over),
            clock: Clock {
                at: moment.at as f64,
                running: false,
                ..self.last.clock
            },
            cursor: Some(k),
            landed: Vec::new(),
            thinkers: Vec::new(),
            thinking: Vec::new(),
            ..self.last.clone()
        }
    }

    /// The choices that land as the cursor steps forward onto bubble `k`:
    /// those its board makes that the board of bubble `k - 1` did not.
    pub fn landed(&self, k: usize) -> Vec<Landed> {
        let before = k.checked_sub(1).map(|j| &self.moments[j]);
        let before = before.and_then(|moment| moment.panel.as_deref());
        match &self.moments[k].panel {
            Some(after) => panel_landings(before, after),
            None => Vec::new(),
        }
    }
}

/// What a board draws: the board, the seats, and the choices flashing on
/// it.
#[derive(Debug, Clone, PartialEq)]
pub struct Board<P = AvalonPanel> {
    /// The board as it stands.
    pub panel: P,
    /// When the game ended, in log time, once it has.
    pub ended: Option<Millis>,
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

    /// The seat square of `player`.
    pub fn mark(&self, player: &str) -> Option<SeatMark> {
        self.seats
            .iter()
            .find(|seat| seat.name == player)
            .map(SeatMark::of)
    }
}

/// A seat square: a small square in its side's color holding the seat
/// number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatMark {
    /// The seat number: `3` for `Player 3`.
    pub number: String,
    /// The side whose color fills it.
    pub side: Side,
}

impl SeatMark {
    /// The square of `seat`.
    pub fn of(seat: &Seat) -> Self {
        Self {
            number: seat_number(&seat.name).to_string(),
            side: seat.role.side(),
        }
    }

    /// Its classes: `sd-seat sd-evil`, and `sd-out` for a player who is
    /// out.
    pub fn class(&self, out: bool) -> String {
        let class = format!("sd-seat sd-{}", side_key(self.side));
        if out { class + " sd-out" } else { class }
    }
}

/// When the dots of the call `cue` sends out are due, in log time, if it
/// sends one out.
pub fn dots_due(cue: &Cue) -> Option<Millis> {
    matches!(cue.beat, Beat::Thinking { .. }).then_some(cue.at + THINKING_DELAY)
}

impl Board<WerewolfPanel> {
    /// The day's tentative decision, its countdown stopped once the day
    /// begins closing or the game ends.
    pub fn decision(&self) -> Option<Decision> {
        let ratify = self.panel.ratify.as_ref();
        let ratify = ratify.filter(|_| self.panel.phase == WerewolfPhase::Day)?;
        Some(Decision {
            target: ratify.target.clone(),
            timer: Timer {
                began: ratify.since,
                limit_ms: ratify.limit_ms,
                stopped: self.panel.closing.or(self.ended),
            },
        })
    }

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

/// The day's tentative decision as the board shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Whom the vote would put out, or `None` for nobody.
    pub target: Option<PlayerId>,
    /// Its ratify countdown.
    pub timer: Timer,
}

impl Decision {
    /// What it says at log time `now`: `Player 4 out in 14s`, or
    /// `Player 4 out` once its countdown has stopped.
    pub fn label(&self, now: f64) -> String {
        let seconds = self
            .timer
            .stopped
            .is_none()
            .then(|| self.timer.seconds(now));
        ratify(self.target.as_deref(), seconds)
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

/// The tentative decision: `Player 4 out in 14s`, or `Nobody out in 14s`,
/// and without `seconds`, `Player 4 out`.
pub fn ratify(target: Option<&str>, seconds: Option<u64>) -> String {
    let out = format!("{} out", target.unwrap_or("Nobody"));
    match seconds {
        Some(seconds) => format!("{out} in {seconds}s"),
        None => out,
    }
}

/// How a player died: `killed night 2`, or `voted out day 1`.
pub fn death(death: &Death) -> String {
    match death.phase {
        WerewolfPhase::Night => format!("killed night {}", death.round),
        WerewolfPhase::Day => format!("voted out day {}", death.round),
    }
}

/// What a seer learned: `Player 6 is a werewolf`.
pub fn learned(learned: &Learned) -> String {
    format!("{} is a {}", learned.player, learned.role)
}

/// How a night turned out: `Wolves chose Player 2, who died`, or
/// `Wolves chose Player 3, protected by Player 1`.
pub fn night(night: &NightOutcome) -> String {
    match (&night.victim, &night.died) {
        (None, _) => "Wolves chose nobody".to_string(),
        (Some(victim), Some(_)) => format!("Wolves chose {victim}, who died"),
        (Some(victim), None) => format!(
            "Wolves chose {victim}, protected by {}",
            night.protected_by.join(" and ")
        ),
    }
}

/// `player`'s seat number: `3` for `Player 3`.
pub fn seat_number(player: &str) -> &str {
    player.trim_start_matches("Player ")
}

/// A heading's title, its first sentence, and the rest of it, told under
/// it: `Quest 3, proposal 2 of 5. Player 4 leads.` is `Quest 3 · proposal
/// 2 of 5` over `Player 4 leads.`
pub fn chapter(heading: &str) -> (String, Option<String>) {
    let (title, rest) = heading.split_once(". ").unwrap_or((heading, ""));
    let title = title.trim_end_matches('.').replace(", ", " · ");
    let rest = rest.trim();
    (title, (!rest.is_empty()).then(|| rest.to_string()))
}

/// What a quest's square says under it: `3 seats`, `now` for the quest
/// underway, and on a line of its own, how many sabotages fail a quest
/// that needs more than one.
pub fn quest_caption(token: &QuestToken, current: bool) -> String {
    let caption = if current {
        "now".to_string()
    } else {
        format!("{} seats", token.size)
    };
    match token.sabotages_needed {
        1 => caption,
        needed => format!("{caption}\n{needed} fails"),
    }
}

/// How a proposal went: `voting`, `rejected`, `approved`, `quest failed`
/// or `quest succeeded`.
pub fn verdict(row: &ProposalRow) -> &'static str {
    match (row.approved, &row.result) {
        (None, _) => "voting",
        (Some(false), _) => "rejected",
        (Some(true), None) => "approved",
        (Some(true), Some(result)) if result.succeeded => "quest succeeded",
        (Some(true), Some(_)) => "quest failed",
    }
}

/// `player`'s choice in `entries`, if made.
pub fn entry<T: Copy>(entries: &[(PlayerId, Option<T>)], player: &str) -> Option<T> {
    entries
        .iter()
        .find(|(name, _)| name == player)
        .and_then(|(_, choice)| *choice)
}

/// The names table for `seats`: evil on the left and good on the right,
/// every bubble unfilled in ink, and the narrator centered and muted.
/// Each player's messages carry their side's colors, `--side` for text
/// and `--side-fill` for a square, for the stylesheet to read.
fn names(seats: &[Seat]) -> Names {
    let narrator = Look::at(Position::Center)
        .background("transparent")
        .foreground("var(--rc-muted)");
    seats
        .iter()
        .fold(Names::none().name(NARRATOR, narrator), |names, seat| {
            let position = match seat.role.side() {
                Side::Evil => Position::Left,
                Side::Good => Position::Right,
            };
            let side = side_key(seat.role.side());
            let look = Look::at(position)
                .background("transparent")
                .foreground("var(--rc-fg)")
                .property("--side", format!("var(--sd-{side}-text)"))
                .property("--side-fill", format!("var(--sd-{side})"));
            names.name(seat.label.clone(), look)
        })
}

/// `side` as the stylesheet's custom properties name it: `--sd-evil`.
pub fn side_key(side: Side) -> &'static str {
    match side {
        Side::Good => "good",
        Side::Evil => "evil",
    }
}

/// The choices the board `after` makes that the board `before` it did
/// not have, a board of the other game counting as none.
fn panel_landings(before: Option<&Panel>, after: &Panel) -> Vec<Landed> {
    match (before, after) {
        (Some(Panel::Avalon(before)), Panel::Avalon(after)) => landings(Some(before), after),
        (_, Panel::Avalon(after)) => landings(None, after),
        (Some(Panel::Werewolf(before)), Panel::Werewolf(after)) => {
            werewolf_landings(Some(before), after)
        }
        (_, Panel::Werewolf(after)) => werewolf_landings(None, after),
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
    use crate::address;
    use social_deduction_gui_types::{
        Assassination, AvalonPhase, AvalonSeat, Deed, ProposalRow, QuestResult, QuestToken, Ratify,
        Role, SeatState,
    };

    /// `beat` at time 0.
    fn cue(beat: Beat) -> Cue {
        Cue { at: 0, beat }
    }

    /// A seat of `role`.
    fn seat(name: &str, role: Role) -> Seat {
        Seat {
            name: name.to_string(),
            label: format!("{name} · {role}"),
            role,
            strategy: "random".to_string(),
        }
    }

    /// The cast of a game of three.
    fn cast() -> Cue {
        cue(Beat::Cast {
            game: GameName::Avalon,
            seats: vec![
                seat("Player 1", Role::Merlin),
                seat("Player 2", Role::Assassin),
                seat("Player 3", Role::Servant),
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
    fn the_cast_sets_the_names_table_with_evil_left_and_good_right_all_unfilled() {
        let screen = shown(&[cast()]);
        assert_eq!(screen.game, Some(GameName::Avalon));
        let look = |name: &str| screen.names.get(name).cloned().unwrap();
        assert_eq!(
            look("Player 1 · Merlin"),
            Look::at(Position::Right)
                .background("transparent")
                .foreground("var(--rc-fg)")
                .property("--side", "var(--sd-good-text)")
                .property("--side-fill", "var(--sd-good)")
        );
        assert_eq!(
            look("Player 2 · Assassin"),
            Look::at(Position::Left)
                .background("transparent")
                .foreground("var(--rc-fg)")
                .property("--side", "var(--sd-evil-text)")
                .property("--side-fill", "var(--sd-evil)")
        );
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
                ("m1", NARRATOR, &Body::Heading("Quest 1".to_string())),
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
        assert_eq!(screen.heading.as_deref(), Some("Quest 1"));
    }

    #[test]
    fn a_heading_is_its_first_sentence_and_the_rest_is_told_under_it() {
        let heading = "Quest 3, proposal 2 of 5. Player 4 leads and needs a team of 2.";
        let screen = shown(&[cast(), cue(Beat::Heading(heading.to_string()))]);
        let bodies: Vec<&Body> = screen.posts.iter().map(|post| &post.body).collect();
        assert_eq!(
            bodies,
            [
                &Body::Heading("Quest 3 · proposal 2 of 5".to_string()),
                &Body::Text("Player 4 leads and needs a team of 2.".to_string()),
            ]
        );
        assert_eq!(screen.heading.as_deref(), Some("Quest 3 · proposal 2 of 5"));
        assert_eq!(chapter("Night 2."), ("Night 2".to_string(), None));
        assert_eq!(chapter("Day 1"), ("Day 1".to_string(), None));
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
    fn playing_a_game_again_from_the_start_ends_on_the_same_screen() {
        let cues = game();
        let mut again = Screen::default();
        let mut flashes = Vec::new();
        for cue in &cues {
            flashes.extend(again.apply(cue, true));
        }
        assert!(!flashes.is_empty());
        for number in flashes {
            again.expire(number);
        }
        assert!(again.clock.running);
        again.clock.running = false;
        assert_eq!(again, shown(&cues));
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
    fn the_phase_timer_is_the_latest_boards_of_either_game() {
        assert_eq!(Screen::default().phase_timer(), None);
        let avalon = shown(&[cast(), panel(&board())]);
        assert_eq!(
            avalon.phase_timer(),
            Some(Timer {
                began: 0,
                limit_ms: 60_000,
                stopped: None,
            })
        );
        let werewolf = shown(&[werewolf_cast(), tonight()]);
        assert_eq!(
            werewolf.phase_timer(),
            Some(Timer {
                began: 1_000,
                limit_ms: 30_000,
                stopped: None,
            })
        );
    }

    /// The four's first day, deciding on `Player 4` from 3 s in for 20 s.
    fn deciding() -> WerewolfPanel {
        let mut day = first_day();
        day.ratify = Some(Ratify {
            target: Some("Player 4".to_string()),
            since: 3_000,
            limit_ms: 20_000,
        });
        day
    }

    /// `panel`, closing from 10 s in.
    fn closing(panel: &WerewolfPanel) -> WerewolfPanel {
        WerewolfPanel {
            closing: Some(10_000),
            ..panel.clone()
        }
    }

    #[test]
    fn a_closing_phase_shows_closing_in_place_of_its_clock_until_the_next_phase() {
        let open = shown(&[werewolf_cast(), werewolf(&deciding())]);
        assert!(!open.closing());
        assert!(open.phase_timer().is_some());
        let closed = shown(&[
            werewolf_cast(),
            werewolf(&deciding()),
            werewolf(&closing(&deciding())),
        ]);
        assert!(closed.closing());
        assert_eq!(closed.phase_timer(), None);
        let night = shown(&[
            werewolf_cast(),
            werewolf(&closing(&deciding())),
            werewolf(&first_night()),
        ]);
        assert!(!night.closing());
        assert!(night.phase_timer().is_some());
        assert!(!shown(&[cast(), panel(&board())]).closing());
    }

    #[test]
    fn a_game_over_shows_neither_clock_nor_closing() {
        let over = cue(Beat::Over {
            outcome: Outcome::Stopped,
        });
        let avalon = shown(&[cast(), panel(&board()), over.clone()]);
        assert_eq!(avalon.phase_timer(), None);
        let night = shown(&[werewolf_cast(), tonight(), over.clone()]);
        assert_eq!(night.phase_timer(), None);
        let closed = shown(&[werewolf_cast(), werewolf(&closing(&deciding())), over]);
        assert!(!closed.closing());
    }

    #[test]
    fn a_closing_day_freezes_its_decision_where_it_stood_when_it_began_closing() {
        let mut later = closing(&deciding());
        later.votes[0].1 = Some("Player 4".to_string());
        let screen = shown(&[
            werewolf_cast(),
            werewolf(&deciding()),
            werewolf(&closing(&deciding())),
            werewolf(&later),
        ]);
        let decision = screen.werewolf().unwrap().decision().unwrap();
        assert_eq!(decision.timer.stopped, Some(10_000));
        assert_eq!(decision.label(40_000.0), "Player 4 out");
        let open = shown(&[werewolf_cast(), werewolf(&deciding())]);
        let decision = open.werewolf().unwrap().decision().unwrap();
        assert_eq!(decision.timer.stopped, None);
        assert_eq!(decision.label(10_000.0), "Player 4 out in 13s");
        let night = WerewolfPanel {
            phase: WerewolfPhase::Night,
            ..deciding()
        };
        let screen = shown(&[werewolf_cast(), werewolf(&night)]);
        assert_eq!(screen.werewolf().unwrap().decision(), None);
    }

    #[test]
    fn a_game_over_stops_the_decision_where_it_stood_when_the_game_ended() {
        let over = Cue {
            at: 12_000,
            beat: Beat::Over {
                outcome: Outcome::Stopped,
            },
        };
        let screen = shown(&[werewolf_cast(), werewolf(&deciding()), over]);
        let decision = screen.werewolf().unwrap().decision().unwrap();
        assert_eq!(decision.timer.stopped, Some(12_000));
        assert_eq!(decision.label(40_000.0), "Player 4 out");
        let closed = shown(&[
            werewolf_cast(),
            werewolf(&closing(&deciding())),
            cue(Beat::Over {
                outcome: Outcome::Stopped,
            }),
        ]);
        let decision = closed.werewolf().unwrap().decision().unwrap();
        assert_eq!(decision.timer.stopped, Some(10_000));
    }

    /// The cast of a Werewolf game of four: a werewolf, a doctor, a seer
    /// and a villager.
    fn werewolf_cast() -> Cue {
        cue(Beat::Cast {
            game: GameName::Werewolf,
            seats: vec![
                seat("Player 1", Role::Werewolf),
                seat("Player 2", Role::Doctor),
                seat("Player 3", Role::Seer),
                seat("Player 4", Role::Villager),
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
            closing: None,
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
        assert_eq!(Screen::default().theme(None), None);
        assert_eq!(shown(&[werewolf_cast()]).theme(None), None);
        assert_eq!(
            shown(&[werewolf_cast(), tonight()]).theme(None),
            Some("dark")
        );
        let day = werewolf(&first_day());
        assert_eq!(
            shown(&[werewolf_cast(), tonight(), day.clone()]).theme(None),
            Some("light")
        );
        let over = cue(Beat::Over {
            outcome: Outcome::Stopped,
        });
        let ended = [werewolf_cast(), day, over];
        // A saved game at rest takes its final phase's theme, and one
        // played through follows the OS once it is over.
        assert_eq!(shown(&ended).theme(None), Some("light"));
        let mut played = Screen::default();
        for cue in &ended {
            played.apply(cue, true);
        }
        assert_eq!(played.theme(None), None);
        assert_eq!(shown(&game()).theme(None), None);
    }

    #[test]
    fn night_and_day_hold_whatever_the_appearance_and_otherwise_the_appearance_holds() {
        let night = shown(&[werewolf_cast(), tonight()]);
        assert_eq!(night.theme(Some("light")), Some("dark"));
        let day = shown(&[werewolf_cast(), tonight(), werewolf(&first_day())]);
        assert_eq!(day.theme(Some("dark")), Some("light"));
        assert_eq!(shown(&game()).theme(Some("dark")), Some("dark"));
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
    fn the_board_writes_its_rows_as_the_console_would_say_them() {
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
        assert_eq!(ratify(Some("Player 4"), Some(14)), "Player 4 out in 14s");
        assert_eq!(ratify(None, Some(3)), "Nobody out in 3s");
        assert_eq!(ratify(Some("Player 4"), None), "Player 4 out");
        assert_eq!(ratify(None, None), "Nobody out");
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
            "Wolves chose Player 2, who died"
        );
        assert_eq!(
            outcome(Some("Player 3"), &["Player 1", "Player 5"], None),
            "Wolves chose Player 3, protected by Player 1 and Player 5"
        );
        assert_eq!(outcome(None, &[], None), "Wolves chose nobody");
        let learned = Learned {
            player: "Player 6".to_string(),
            role: Role::Werewolf,
            night: 1,
        };
        assert_eq!(super::learned(&learned), "Player 6 is a werewolf");
    }

    #[test]
    fn the_boards_player_marks_are_side_colored_squares_numbered_from_the_seat() {
        let screen = shown(&[cast(), panel(&board())]);
        let board = screen.board().unwrap();
        let mark = board.mark("Player 2").unwrap();
        assert_eq!(
            mark,
            SeatMark {
                number: "2".to_string(),
                side: Side::Evil
            }
        );
        assert_eq!(mark.class(false), "sd-seat sd-evil");
        assert_eq!(
            board.mark("Player 3").unwrap().class(true),
            "sd-seat sd-good sd-out"
        );
        assert_eq!(SeatMark::of(&screen.seats[0]).number, "1");
        assert_eq!(board.mark("Player 9"), None);
        assert_eq!(seat_number("Player 10"), "10");
    }

    #[test]
    fn a_players_label_finds_their_seat() {
        let screen = shown(&[cast()]);
        assert_eq!(
            screen.seat("Player 2 · Assassin"),
            Some(screen.seats[1].clone())
        );
        assert_eq!(screen.seat(NARRATOR), None);
    }

    #[test]
    fn each_quest_is_captioned_with_its_seats_or_now() {
        let token = |sabotages_needed| QuestToken {
            size: 3,
            sabotages_needed,
            result: None,
        };
        assert_eq!(quest_caption(&token(1), false), "3 seats");
        assert_eq!(quest_caption(&token(1), true), "now");
        assert_eq!(quest_caption(&token(2), false), "3 seats\n2 fails");
    }

    #[test]
    fn each_proposal_row_says_how_it_went() {
        let mut row = board().proposals[0].clone();
        assert_eq!(verdict(&row), "voting");
        row.approved = Some(false);
        assert_eq!(verdict(&row), "rejected");
        row.approved = Some(true);
        assert_eq!(verdict(&row), "approved");
        let result = |succeeded| {
            Some(QuestResult {
                team: vec![],
                sabotages: 0,
                succeeded,
            })
        };
        row.result = result(true);
        assert_eq!(verdict(&row), "quest succeeded");
        row.result = result(false);
        assert_eq!(verdict(&row), "quest failed");
    }

    #[test]
    fn each_side_has_its_colors_and_no_role_has_any() {
        assert_eq!(side_key(Side::Good), "good");
        assert_eq!(side_key(Side::Evil), "evil");
        let css = include_str!("../style.css");
        for key in ["good", "evil"] {
            assert!(css.contains(&format!("--sd-{key}:")), "{key}");
            assert!(css.contains(&format!("--sd-{key}-text:")), "{key}");
        }
        assert!(!css.contains("--role-"));
    }

    /// The cues of a random game played from the example `config`, such
    /// as `werewolf/random.toml`.
    async fn random(config: &str) -> Vec<Cue> {
        let path = format!("{}/../cli/configs/{config}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap();
        let environment = free_agent_model::spec::Environment;
        let prepared = social_deduction::run::prepare(&text, &environment)
            .await
            .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let playing = social_deduction::live::play(prepared, move |cue| Ok(sender.send(cue)?));
        playing.finished().await;
        receiver.try_iter().collect()
    }

    /// The cues of the recorded game `name` in `fixtures/`.
    fn recorded(name: &str) -> Vec<Cue> {
        let path = format!("{}/fixtures/{name}.jsonl", env!("CARGO_MANIFEST_DIR"));
        let log = std::fs::read_to_string(&path).unwrap();
        let mut read = social_deduction::stage::recordings(&log).unwrap();
        read.games.remove(0).cues
    }

    /// Every test game: two random games of each kind and the recorded
    /// prompted ones.
    async fn test_games() -> Vec<(String, Vec<Cue>)> {
        let mut games = Vec::new();
        for config in ["avalon/random.toml", "werewolf/random.toml"] {
            for round in 0..2 {
                games.push((format!("{config} {round}"), random(config).await));
            }
        }
        for name in address::RECORDED {
            games.push((name.to_string(), recorded(name)));
        }
        games
    }

    /// The screen a fresh `Screen` shows after every cue up to the first
    /// to post once bubble `k` is posted: the screen while bubble `k` is
    /// the latest.
    fn latest_at(cues: &[Cue], k: usize) -> Screen {
        let mut screen = Screen::default();
        for cue in cues {
            let mut next = screen.clone();
            next.apply(cue, false);
            if screen.posts.len() > k && next.posts.len() > screen.posts.len() {
                break;
            }
            screen = next;
        }
        screen
    }

    #[tokio::test(start_paused = true)]
    async fn the_screen_at_each_bubble_is_the_screen_folded_until_the_next_bubble() {
        for (name, cues) in test_games().await {
            let timeline = Timeline::new(&cues);
            let last = shown(&cues);
            assert!(!timeline.is_empty(), "{name}");
            assert_eq!(timeline.len(), last.posts.len(), "{name}");
            for k in 0..timeline.len() {
                let screen = timeline.screen(k);
                let folded = latest_at(&cues, k);
                assert_eq!(screen.panel, folded.panel, "{name} {k}");
                assert_eq!(screen.heading, folded.heading, "{name} {k}");
                assert_eq!(screen.outcome, folded.outcome, "{name} {k}");
                assert_eq!(screen.ended, folded.ended, "{name} {k}");
                assert_eq!(screen.clock, folded.clock, "{name} {k}");
                assert_eq!(screen.posts, last.posts, "{name} {k}");
                assert_eq!(screen.cursor, Some(k), "{name} {k}");
                assert!(screen.thinkers.is_empty(), "{name} {k}");
                assert!(screen.landed.is_empty(), "{name} {k}");
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn moments_under_one_board_share_it_and_the_last_is_the_screen_at_rest() {
        for (name, cues) in test_games().await {
            let timeline = Timeline::new(&cues);
            let moments = timeline.moments();
            for pair in moments.windows(2) {
                if let (Some(before), Some(after)) = (&pair[0].panel, &pair[1].panel) {
                    assert_eq!(before == after, Arc::ptr_eq(before, after), "{name}");
                }
            }
            let shared = moments.windows(2).filter(|pair| match &pair[..] {
                [Moment { panel: Some(a), .. }, Moment { panel: Some(b), .. }] => Arc::ptr_eq(a, b),
                _ => false,
            });
            assert!(shared.count() > 0, "{name}");
            let at_rest = timeline.screen(timeline.len() - 1);
            assert_eq!(
                Screen {
                    cursor: None,
                    ..at_rest
                },
                shown(&cues),
                "{name}"
            );
        }
    }

    #[test]
    fn a_bubble_that_tells_a_death_shows_the_board_after_it() {
        let mut dead = first_night();
        dead.seats[3].died = Some(Death {
            round: 1,
            phase: WerewolfPhase::Night,
        });
        let cues = [
            werewolf_cast(),
            cue(Beat::Heading("Night 1.".to_string())),
            tonight(),
            cue_at(5_000, Beat::Told(vec!["Player 4 is killed.".to_string()])),
            cue_at(5_000, Beat::Panel(Panel::Werewolf(dead.clone()))),
            cue_at(
                8_000,
                Beat::Heading("Day 1. The village wakes.".to_string()),
            ),
        ];
        let timeline = Timeline::new(&cues);
        assert_eq!(timeline.len(), 4);
        let night = timeline.screen(0);
        assert_eq!(night.heading.as_deref(), Some("Night 1"));
        assert_eq!(night.panel, Some(Panel::Werewolf(first_night())));
        assert_eq!(night.clock.at, 0.0);
        let killed = timeline.screen(1);
        assert_eq!(killed.panel, Some(Panel::Werewolf(dead.clone())));
        assert_eq!(killed.clock.at, 5_000.0);
        assert_eq!(timeline.landed(1), [Landed::Death("Player 4".to_string())]);
        // A heading and the words told under it are one chapter.
        assert_eq!(timeline.screen(2).heading.as_deref(), Some("Day 1"));
        assert_eq!(timeline.screen(3).heading.as_deref(), Some("Day 1"));
        assert_eq!(timeline.screen(3).clock.at, 8_000.0);
        assert!(timeline.landed(2).is_empty());
        assert!(timeline.landed(0).is_empty());
    }

    #[test]
    fn the_outcome_shows_only_from_the_end_of_the_game() {
        let timeline = Timeline::new(&game());
        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline.screen(1).outcome, None);
        assert!(!timeline.moments()[1].over);
        assert_eq!(timeline.screen(2).outcome, shown(&game()).outcome);
        assert!(timeline.moments()[2].over);
        // The first bubble is posted under a board with nothing chosen,
        // and the second is followed by a team proposed and a ballot cast.
        assert!(timeline.landed(0).is_empty());
        assert_eq!(timeline.landed(1).len(), 2);
        assert!(timeline.landed(2).is_empty());
    }

    #[test]
    fn a_board_of_the_other_game_or_none_lands_as_if_first() {
        let mut voted = first_day();
        voted.votes[0].1 = Some("Player 2".to_string());
        let said = |at| {
            cue_at(
                at,
                Beat::Said {
                    who: "Player 1".to_string(),
                    text: "Hi.".to_string(),
                },
            )
        };
        let cues = [
            said(0),
            said(1),
            panel(&board()),
            said(2),
            werewolf(&voted),
            said(3),
        ];
        let timeline = Timeline::new(&cues);
        assert_eq!(timeline.screen(0).panel, None);
        assert!(timeline.landed(0).is_empty());
        assert!(timeline.landed(1).is_empty());
        assert_eq!(timeline.landed(2), [Landed::Vote("Player 1".to_string())]);
        assert!(Timeline::new(&[]).is_empty());
    }

    #[test]
    fn a_screen_under_review_shows_in_the_era_it_replaces_with_what_lands_flashing() {
        let cues = recorded("werewolf-prompted");
        let timeline = Timeline::new(&cues);
        let mut screen = Screen::default();
        screen.reset();
        let k = (1..timeline.len())
            .find(|&k| !timeline.landed(k).is_empty())
            .unwrap();
        let number = screen.review(timeline.screen(k), timeline.landed(k));
        assert_eq!(screen.era(), 1);
        assert_eq!(screen.cursor, Some(k));
        let landed: Vec<Landed> = screen
            .landed
            .iter()
            .map(|(_, landed)| landed.clone())
            .collect();
        assert_eq!(landed, timeline.landed(k));
        screen.expire(number.unwrap());
        assert!(screen.landed.is_empty());
        let later = screen.review(timeline.screen(k), timeline.landed(k));
        assert_ne!(later, number);
        assert_eq!(screen.review(timeline.screen(0), Vec::new()), None);
        assert!(screen.landed.is_empty());
    }

    #[test]
    fn a_bubble_is_found_by_its_key_in_the_chat() {
        let screen = shown(&game());
        for (k, post) in screen.posts.iter().enumerate() {
            assert_eq!(bubble(&key(3, &post.id)), Some(k));
        }
        assert_eq!(key(3, "m12"), "3-m12");
        assert_eq!(bubble("2-m0"), None);
        assert_eq!(bubble("m1"), None);
        assert_eq!(bubble("think-Player 3"), None);
    }

    #[test]
    fn a_timeline_knows_its_chapters_and_how_the_game_ended() {
        let cues = recorded("werewolf-prompted");
        let timeline = Timeline::new(&cues);
        let last = timeline.screen(timeline.len() - 1);
        let headings: Vec<usize> = (0..last.posts.len())
            .filter(|&k| matches!(last.posts[k].body, Body::Heading(_)))
            .collect();
        assert!(!headings.is_empty());
        assert_eq!(timeline.chapters(), headings);
        assert_eq!(timeline.outcome(), last.outcome.as_ref());
        assert_eq!(Timeline::new(&[]).outcome(), None);
    }

    #[test]
    fn live_play_has_no_cursor() {
        assert_eq!(Screen::default().cursor, None);
        assert_eq!(shown(&game()).cursor, None);
    }
}
