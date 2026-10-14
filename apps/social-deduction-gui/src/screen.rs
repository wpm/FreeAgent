//! What the screen shows, as a pure function of the cues so far.
//!
//! [`Screen::apply`] folds one cue at a time into the chat, the names
//! table, the header and the board. The view draws a `Screen` and holds
//! no state of its own, so the whole viewer is tested here, natively.

use leptos_rich_chat::{Look, Names, Position};
use social_deduction_stage::{
    AvalonPanel, Beat, Cue, GameName, Millis, Outcome, Panel, PlayerId, Role, Seat, Side,
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
    /// Log milliseconds per viewer millisecond.
    pub rate: f64,
    /// Every call out whose dots have not shown, with when it went out.
    thinking: Vec<(PlayerId, Millis)>,
    /// How many cues have been applied.
    applied: usize,
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
            rate: 1.0,
            thinking: Vec::new(),
            applied: 0,
        }
    }
}

impl Screen {
    /// Show `cue`. With `flash`, every choice a new board makes lands, and
    /// the cue's number comes back for [`Screen::expire`] to end the
    /// flash; without it, as for a saved game shown at once, nothing
    /// lands. Every call out longer than [`THINKING_DELAY`] by the cue's
    /// time shows its dots.
    pub fn apply(&mut self, cue: &Cue, flash: bool) -> Option<usize> {
        self.reveal(cue.at);
        self.applied += 1;
        let number = self.applied;
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
            Beat::Clock { rate } => self.rate = *rate,
            Beat::Panel(panel) => {
                let landed = match (flash, &self.panel, panel) {
                    (true, before, Panel::Avalon(after)) => {
                        let before = match before {
                            Some(Panel::Avalon(before)) => Some(before),
                            _ => None,
                        };
                        landings(before, after)
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

    /// End the flash of what the cue numbered `number` landed.
    pub fn expire(&mut self, number: usize) {
        self.landed.retain(|(landed_by, _)| *landed_by != number);
    }

    /// What the Avalon board draws, once there is one.
    pub fn board(&self) -> Option<Board> {
        match &self.panel {
            Some(Panel::Avalon(panel)) => Some(Board {
                panel: panel.clone(),
                seats: self.seats.clone(),
                landed: self
                    .landed
                    .iter()
                    .map(|(_, landed)| landed.clone())
                    .collect(),
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

/// What the Avalon board draws: the board, the seats, and the choices
/// flashing on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    /// The board as it stands.
    pub panel: AvalonPanel,
    /// Every seat, in seat order.
    pub seats: Vec<Seat>,
    /// The choices flashing now.
    pub landed: Vec<Landed>,
}

impl Board {
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

#[cfg(test)]
mod tests {
    use super::*;
    use social_deduction_stage::{
        Assassination, AvalonPhase, AvalonSeat, Deed, ProposalRow, QuestToken,
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
    fn the_clock_sets_the_rate() {
        assert_eq!(Screen::default().rate, 1.0);
        let screen = shown(&[cue(Beat::Clock { rate: 4.0 })]);
        assert_eq!(screen.rate, 4.0);
    }

    #[test]
    fn late_bubbles_are_styled() {
        let css = include_str!("../style.css");
        assert!(css.contains(".sd-late"));
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
