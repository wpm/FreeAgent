//! The page: a header, the chat beside the board, and a footer naming the
//! commit the page was built from. Every part draws the [`Screen`].

use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::{Message, RenderOptions, RichChatStyle, RichText, Transcript};
use social_deduction_stage::{
    Cue, Deed, GameName, NightDeed, Outcome, Side, WerewolfPanel, WerewolfPhase,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::screen::{
    self, Board, Body, Landed, NARRATOR, Post, Screen, THINKING_DELAY, Thinker, Timer, dots_due,
    entry, role_key,
};

/// How often a replay shows the next cue.
const STEP: Duration = Duration::from_millis(250);

/// How long a choice flashes as it lands.
const FLASH: Duration = Duration::from_millis(900);

/// How often the clocks redraw.
const TICK: Duration = Duration::from_millis(100);

/// The commit the page was built from, when the build names one.
const COMMIT: Option<&str> = option_env!("GITHUB_SHA");

/// The games the page shows, each by the name of its cues file and its
/// URL hash.
const GAMES: [(&str, &str); 2] = [("avalon", "Avalon"), ("werewolf", "Werewolf")];

/// The viewer: it fetches the cues of the game the URL hash names,
/// `#avalon` (the default) or `#werewolf`, from beside the page and shows
/// the whole game at once. **Play** clears the screen and replays the
/// cues, one every 250 ms.
#[component]
pub fn App() -> impl IntoView {
    let screen = RwSignal::new(Screen::default());
    let cues = StoredValue::new(Vec::<Cue>::new());
    let failure = RwSignal::new(None::<String>);
    let replayed = RwSignal::new(false);
    let replay = StoredValue::new(None::<IntervalHandle>);
    let game = RwSignal::new(hashed());
    // The viewer's time, now and when the latest cue arrived.
    let tick = RwSignal::new(elapsed());
    let arrived = StoredValue::new(elapsed());

    let stop = move || {
        if let Some(handle) = replay.get_value() {
            handle.clear();
        }
    };

    let _ = window_event_listener(ev::hashchange, move |_| game.set(hashed()));
    Effect::new(move |_| {
        let name = game.get();
        stop();
        screen.set(Screen::default());
        replayed.set(false);
        failure.set(None);
        spawn_local(async move {
            let fetched = fetched(name).await;
            if game.get_untracked() != name {
                return;
            }
            match fetched {
                Ok(fetched) => {
                    screen.update(|screen| {
                        for cue in &fetched {
                            screen.apply(cue, false);
                        }
                    });
                    cues.set_value(fetched);
                }
                Err(error) => failure.set(Some(error)),
            }
        });
    });

    let _ = set_interval_with_handle(move || tick.set(elapsed()), TICK);
    // A memo, so that a stopped clock redraws nothing as it ticks.
    let now = Signal::from(Memo::new(move |_| {
        let waited = tick.get() - arrived.get_value();
        screen.with(|screen| screen.clock.now(waited))
    }));

    let theme = Memo::new(move |_| screen.with(Screen::theme));
    Effect::new(move |_| {
        if let Some(root) = document().document_element() {
            let _ = match theme.get() {
                Some(theme) => root.set_attribute("data-theme", theme),
                None => root.remove_attribute("data-theme"),
            };
        }
    });

    let play = move |_| {
        stop();
        screen.set(Screen::default());
        replayed.set(true);
        let next = StoredValue::new(0usize);
        let handle = set_interval_with_handle(
            move || {
                let index = next.get_value();
                let Some(cue) = cues.with_value(|cues| cues.get(index).cloned()) else {
                    if let Some(handle) = replay.get_value() {
                        handle.clear();
                    }
                    return;
                };
                next.set_value(index + 1);
                let at = elapsed();
                let waited = at - arrived.get_value();
                arrived.set_value(at);
                let applied = screen.try_update(|screen| {
                    screen.advance(waited);
                    screen.apply(&cue, true)
                });
                if let Some(Some(number)) = applied {
                    set_timeout(move || screen.update(|screen| screen.expire(number)), FLASH);
                }
                if let Some(due) = dots_due(&cue) {
                    let rate = screen.with_untracked(|screen| screen.clock.rate);
                    let delay = Duration::from_millis(THINKING_DELAY).div_f64(rate);
                    set_timeout(move || screen.update(|screen| screen.reveal(due)), delay);
                }
            },
            STEP,
        );
        replay.set_value(handle.ok());
    };

    let options = RenderOptions {
        math: false,
        highlight: false,
        images: false,
        ..RenderOptions::default()
    };
    // Memos, so that a cue that leaves the chat, the names or the board as
    // they were redraws none of them.
    let names = Memo::new(move |_| screen.with(|screen| screen.names.clone()));
    let posts = Memo::new(move |_| screen.with(|screen| screen.posts.clone()));
    let thinkers = Memo::new(move |_| screen.with(|screen| screen.thinkers.clone()));
    let board = Memo::new(move |_| screen.with(Screen::board));
    let werewolf = Memo::new(move |_| screen.with(Screen::werewolf));
    let timer = Memo::new(move |_| screen.with(Screen::phase_timer));
    let rendering = options.clone();
    // Every thinker's dots share one text, which stays empty.
    let silence = RwSignal::new(String::new());
    let messages = Signal::derive(move || {
        let mut messages = posts.with(|posts| messages(posts, &rendering));
        thinkers.with(|thinkers| {
            messages.extend(thinkers.iter().map(|thinker| dots(thinker, silence)));
        });
        messages
    });

    view! {
        <RichChatStyle highlight=false fonts=false names=names />
        <div class="sd-app">
            <header class="sd-header">
                <h1>{move || screen.with(|screen| game_name(screen.game))}</h1>
                <p class="sd-phase">
                    {move || screen.with(|screen| screen.heading.clone())}
                    {move || timer.get().map(|timer| view! { <PhaseClock timer=timer now=now /> })}
                </p>
                {move || screen.with(|screen| screen.outcome.clone()).map(outcome)}
                <nav class="sd-games">
                    {GAMES
                        .map(|(name, title)| {
                            let current = move || (game.get() == name).then_some("page");
                            view! {
                                <a href=format!("#{name}") aria-current=current>
                                    {title}
                                </a>
                            }
                        })
                        .collect_view()}
                </nav>
                <button on:click=play>
                    {move || if replayed.get() { "Replay" } else { "Play" }}
                </button>
            </header>
            <main class="sd-main">
                <div class="sd-chat">
                    {move || failure.get().map(|error| view! { <p class="sd-error">{error}</p> })}
                    <Transcript messages=messages show_names=true options=options />
                </div>
                <aside class="sd-board">
                    {move || board.get().map(|board| view! { <AvalonBoard board=board /> })}
                    {move || {
                        werewolf.get().map(|board| view! { <WerewolfBoard board=board now=now /> })
                    }}
                </aside>
            </main>
            <footer class="sd-footer">{footer()}</footer>
        </div>
    }
}

/// The game the URL hash names, or else the first of the [`GAMES`].
fn hashed() -> &'static str {
    let hash = window().location().hash().unwrap_or_default();
    GAMES
        .iter()
        .map(|(name, _)| *name)
        .find(|name| hash.strip_prefix('#') == Some(name))
        .unwrap_or(GAMES[0].0)
}

/// The viewer's time in milliseconds, from the page's own clock.
fn elapsed() -> f64 {
    window()
        .performance()
        .map_or(0.0, |performance| performance.now())
}

/// The cues of the game `name`, in `name.json` beside the page.
async fn fetched(name: &str) -> Result<Vec<Cue>, String> {
    let file = format!("{name}.json");
    let failed = |error: JsValue| format!("{file}: {error:?}");
    let response: web_sys::Response = JsFuture::from(window().fetch_with_str(&file))
        .await
        .map_err(failed)?
        .dyn_into()
        .map_err(failed)?;
    if !response.ok() {
        return Err(format!("{file}: {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(failed)?)
        .await
        .map_err(failed)?;
    serde_json::from_str(&text.as_string().unwrap_or_default())
        .map_err(|error| format!("{file}: {error}"))
}

/// A bar draining over `timer` as log time runs on.
#[component]
fn Countdown(timer: Timer, now: Signal<f64>) -> impl IntoView {
    let width = move || format!("width: {:.2}%", timer.share(now.get()) * 100.0);
    view! {
        <span class="sd-countdown">
            <span class="sd-countdown-left" style=width></span>
        </span>
    }
}

/// The whole seconds `timer` has left as log time runs on, which change
/// once a second.
fn seconds(timer: Timer, now: Signal<f64>) -> Memo<u64> {
    Memo::new(move |_| timer.seconds(now.get()))
}

/// The phase's clock: a draining bar and the seconds left.
#[component]
fn PhaseClock(timer: Timer, now: Signal<f64>) -> impl IntoView {
    view! {
        <span class="sd-clock">
            <Countdown timer=timer now=now />
            <span>{move || format!("{}s", seconds(timer, now).get())}</span>
        </span>
    }
}

/// `posts`, as Rich-Chat messages, rendered with `options`.
fn messages(posts: &[Post], options: &RenderOptions) -> Vec<Message> {
    posts
        .iter()
        .map(|post| match &post.body {
            Body::Heading(text) => {
                let text = text.clone();
                Message::view(post.id.clone(), NARRATOR, move || {
                    view! { <div class="sd-heading">{text.clone()}</div> }
                })
            }
            Body::Text(text) => Message::new(post.id.clone(), post.name.clone(), text.clone()),
            Body::Late(text) => {
                let text = text.clone();
                let options = options.clone();
                Message::view(post.id.clone(), post.name.clone(), move || {
                    view! {
                        <div class="rc-bubble sd-late">
                            <RichText content=text.clone() options=options.clone() />
                            <p class="sd-late-caption">"Too late, never sent."</p>
                        </div>
                    }
                })
            }
        })
        .collect()
}

/// `thinker`'s dots, `think-Player 3`: a live message whose text,
/// `silence`, is empty.
fn dots(thinker: &Thinker, silence: RwSignal<String>) -> Message {
    let id = format!("think-{}", thinker.who);
    Message::streaming(id, thinker.name.clone(), silence)
}

/// The game's name for the header.
fn game_name(game: Option<GameName>) -> &'static str {
    match game {
        Some(GameName::Avalon) => "Avalon",
        Some(GameName::Werewolf) => "Werewolf",
        None => "Social Deduction",
    }
}

/// How the game ended, in the winning side's color.
fn outcome(outcome: Outcome) -> impl IntoView {
    let (class, text) = match outcome {
        Outcome::Won {
            side: Side::Good,
            how,
        } => ("sd-outcome sd-good", how),
        Outcome::Won {
            side: Side::Evil,
            how,
        } => ("sd-outcome sd-evil", how),
        Outcome::Stopped => ("sd-outcome", "Stopped.".to_string()),
        Outcome::Failed(error) => ("sd-outcome", format!("Failed: {error}")),
    };
    view! { <p class=class>{text}</p> }
}

/// The commit the page was built from, linked, or that it was built
/// locally.
fn footer() -> impl IntoView {
    match COMMIT {
        Some(commit) => {
            let href = format!("https://github.com/wpm/FreeAgent/commit/{commit}");
            let short: String = commit.chars().take(7).collect();
            view! { "Built from commit " <a href=href>{short}</a> }.into_any()
        }
        None => view! { "Built locally" }.into_any(),
    }
}

/// `player` as the board writes it: `Player 3` is `P3`.
fn short(player: &str) -> String {
    player.replace("Player ", "P")
}

/// The custom properties that give `player`'s role colors to a flash.
fn chooser<P>(board: &Board<P>, player: &str) -> String {
    board
        .seat(player)
        .map(|seat| {
            let role = role_key(seat.role);
            format!(
                "--sd-chooser-bg: var(--role-{role}-bg); --sd-chooser-fg: var(--role-{role}-fg);"
            )
        })
        .unwrap_or_default()
}

/// The style of a swatch in `player`'s role colors.
fn swatch<P>(board: &Board<P>, player: &str) -> String {
    board
        .seat(player)
        .map(|seat| {
            let role = role_key(seat.role);
            format!("background: var(--role-{role}-bg); border-color: var(--role-{role}-fg);")
        })
        .unwrap_or_default()
}

/// Whether `choice`, which `player` made, is landing, and the style that
/// flashes it in `player`'s colors when it is.
fn flash<P>(board: &Board<P>, choice: &Landed, player: &str) -> (bool, String) {
    let landing = board.is_landed(choice);
    let style = if landing {
        chooser(board, player)
    } else {
        String::new()
    };
    (landing, style)
}

/// The Avalon board: the quests, the rejected proposals, the team on the
/// table, every vote so far, the players and the assassination.
#[component]
fn AvalonBoard(board: Board) -> impl IntoView {
    let panel = &board.panel;
    let rejected = panel
        .proposals
        .iter()
        .filter(|row| row.quest == panel.quest && row.approved == Some(false))
        .count();

    let quests = panel
        .quests
        .iter()
        .enumerate()
        .map(|(index, token)| {
            let current = usize::from(panel.quest) == index + 1 && token.result.is_none();
            let (state, mark) = match &token.result {
                Some(result) if result.succeeded => (" sd-succeeded", "✓".to_string()),
                Some(result) => (" sd-failed", result.sabotages.to_string()),
                None => ("", token.size.to_string()),
            };
            let class = format!(
                "sd-token{state}{}",
                if current { " sd-current" } else { "" }
            );
            let fails = (token.sabotages_needed == 2).then(|| view! { <small>"2 fail"</small> });
            view! { <div class=class><span>{mark}</span>{fails}</div> }
        })
        .collect_view();

    let pips = (1..=5)
        .map(|pip| {
            let mut class = String::from("sd-pip");
            if pip == 5 {
                class.push_str(" sd-last");
            }
            if pip <= rejected {
                class.push_str(" sd-filled");
            }
            view! { <span class=class></span> }
        })
        .collect_view();

    let team = panel.team.clone().unwrap_or_default();
    let (by_team, team_style) = flash(&board, &Landed::Team, &panel.leader);
    let chips = team
        .iter()
        .map(|member| {
            let deed = entry(&panel.deeds, member).map(|deed| match deed {
                Deed::Support => " ✓",
                Deed::Sabotage => " ✗",
            });
            let (by_deed, deed_style) = flash(&board, &Landed::Deed(member.clone()), member);
            let class = landed_class("sd-chip", by_deed || by_team);
            let style = if by_deed {
                deed_style
            } else {
                team_style.clone()
            };
            view! {
                <span class=class style=style>
                    <span class="sd-swatch" style=swatch(&board, member)></span>
                    {member.clone()}
                    {deed}
                </span>
            }
        })
        .collect_view();

    let header = panel
        .seats
        .iter()
        .map(|seat| view! { <th>{short(&seat.name)}</th> })
        .collect_view();
    let rows = panel
        .proposals
        .iter()
        .map(|row| {
            let ballots = panel
                .seats
                .iter()
                .map(|seat| {
                    let seat = &seat.name;
                    let mark = match entry(&row.ballots, seat) {
                        Some(true) => "✓",
                        Some(false) => "✗",
                        None => "",
                    };
                    let landed = Landed::Ballot {
                        quest: row.quest,
                        proposal: row.proposal,
                        player: seat.clone(),
                    };
                    let (landing, style) = flash(&board, &landed, seat);
                    let class = landed_class("", landing);
                    view! { <td class=class style=style>{mark}</td> }
                })
                .collect_view();
            let result = match (row.approved, &row.result) {
                (Some(false), _) => "no",
                (Some(true), Some(result)) if result.succeeded => "passed",
                (Some(true), Some(_)) => "failed",
                _ => "…",
            };
            let name = format!("{}.{} · {}", row.quest, row.proposal, short(&row.leader));
            view! { <tr><th>{name}</th>{ballots}<td>{result}</td></tr> }
        })
        .collect_view();

    let players = board
        .seats
        .iter()
        .map(|seat| {
            let sees = panel
                .seats
                .iter()
                .find(|board| board.name == seat.name)
                .filter(|board| !board.sees.is_empty())
                .map(|board| {
                    let seen: Vec<String> = board
                        .sees
                        .iter()
                        .map(|player| player.replace("Player ", ""))
                        .collect();
                    format!(", sees {}", seen.join(", "))
                });
            view! {
                <li>
                    <span class="sd-swatch" style=swatch(&board, &seat.name)></span>
                    <span>{seat.name.clone()}</span>
                    <span class="sd-detail">
                        {format!("{}, {}", seat.role, seat.strategy)}
                        {sees}
                    </span>
                </li>
            }
        })
        .collect_view();

    let assassination = panel.assassination.clone().map(|shot| {
        let (landing, style) = flash(&board, &Landed::Target, &shot.assassin);
        let target = shot.target.clone().unwrap_or_else(|| "…".to_string());
        let verdict = shot
            .merlin
            .map(|merlin| if merlin { ": Merlin" } else { ": not Merlin" });
        let class = landed_class("sd-assassination", landing);
        view! {
            <div class=class style=style>
                {format!("{} names {target}", shot.assassin)}
                {verdict}
            </div>
        }
    });

    view! {
        <h2>"Quests"</h2>
        <div class="sd-quests">{quests}</div>
        <h2>"Rejected proposals"</h2>
        <div class="sd-pips">{pips}</div>
        <h2>"On the table"</h2>
        <p class="sd-leader">{format!("♛ {} leads", panel.leader)}</p>
        <div class="sd-chips">{chips}</div>
        <h2>"Votes so far"</h2>
        <div class="sd-votes-scroll">
            <table class="sd-votes">
                <thead><tr><th></th>{header}<th></th></tr></thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
        <h2>"Players"</h2>
        <ul class="sd-players">{players}</ul>
        {assassination}
    }
}

/// `class`, landing when `landing` is.
fn landed_class(class: &str, landing: bool) -> String {
    match (class, landing) {
        (_, false) => class.to_string(),
        ("", true) => "sd-landed".to_string(),
        (_, true) => format!("{class} sd-landed"),
    }
}

/// The Werewolf board: the tentative decision and the vote by day, the
/// night roles' deeds by night, the players, what each seer knows, and
/// every night so far.
#[component]
fn WerewolfBoard(board: Board<WerewolfPanel>, now: Signal<f64>) -> impl IntoView {
    let panel = &board.panel;
    let day = panel.phase == WerewolfPhase::Day;

    let decision = panel.ratify.clone().filter(|_| day).map(|ratify| {
        let timer = Timer {
            began: ratify.since,
            limit_ms: ratify.limit_ms,
        };
        let chosen = ratify.target.clone().unwrap_or_default();
        let (landing, style) = flash(&board, &Landed::Ratify, &chosen);
        let target = ratify.target;
        let left = seconds(timer, now);
        let text = move || screen::ratify(target.as_deref(), left.get());
        view! {
            <h2>"Tentative decision"</h2>
            <div class=landed_class("sd-decision", landing) style=style>
                <span>{text}</span>
                <Countdown timer=timer now=now />
            </div>
        }
    });

    let votes = day.then(|| {
        let rows = panel
            .votes
            .iter()
            .map(|(player, target)| {
                let (landing, style) = flash(&board, &Landed::Vote(player.clone()), player);
                view! {
                    <li class=landed_class("sd-row", landing) style=style>
                        <span class="sd-swatch" style=swatch(&board, player)></span>
                        {screen::vote(player, target.as_deref())}
                    </li>
                }
            })
            .collect_view();
        let chips = board
            .tally()
            .into_iter()
            .map(|(target, count)| {
                view! {
                    <span class="sd-chip">
                        <span class="sd-swatch" style=swatch(&board, &target)></span>
                        {format!("{target} · {count}")}
                    </span>
                }
            })
            .collect_view();
        view! {
            <h2>"The vote"</h2>
            <ul class="sd-rows">{rows}</ul>
            <div class="sd-chips">{chips}</div>
        }
    });

    let tonight = (!day && !panel.night.is_empty()).then(|| {
        let rows = panel
            .night
            .iter()
            .map(|(player, deed)| {
                let learned = match deed {
                    NightDeed::Ask(target) => panel
                        .seers
                        .iter()
                        .filter(|(seer, _)| seer == player)
                        .flat_map(|(_, learned)| learned)
                        .find(|learned| learned.night == panel.round && learned.player == *target)
                        .map(|learned| format!("· {}", learned.role)),
                    _ => None,
                };
                let (landing, style) = flash(&board, &Landed::Night(player.clone()), player);
                view! {
                    <li class=landed_class("sd-row", landing) style=style>
                        <span class="sd-swatch" style=swatch(&board, player)></span>
                        {screen::deed(player, deed)}
                        <span class="sd-detail">{learned}</span>
                    </li>
                }
            })
            .collect_view();
        view! {
            <h2>"Tonight"</h2>
            <ul class="sd-rows">{rows}</ul>
        }
    });

    let players = board
        .seats
        .iter()
        .map(|seat| {
            let died = panel
                .seats
                .iter()
                .find(|state| state.name == seat.name)
                .and_then(|state| state.died);
            let (landing, style) = flash(&board, &Landed::Death(seat.name.clone()), &seat.name);
            let class = landed_class(if died.is_some() { "sd-dead" } else { "" }, landing);
            let died = died.map(|died| format!(", {}", screen::death(&died)));
            view! {
                <li class=class style=style>
                    <span class="sd-swatch" style=swatch(&board, &seat.name)></span>
                    <span class="sd-name">{seat.name.clone()}</span>
                    <span class="sd-detail">
                        {format!("{}, {}", seat.role, seat.strategy)}
                        {died}
                    </span>
                </li>
            }
        })
        .collect_view();

    let seers = panel
        .seers
        .iter()
        .filter(|(_, learned)| !learned.is_empty())
        .map(|(seer, learned)| {
            let lines = learned
                .iter()
                .map(|learned| view! { <li>{screen::learned(learned)}</li> })
                .collect_view();
            view! {
                <p class="sd-seer">
                    <span class="sd-swatch" style=swatch(&board, seer)></span>
                    {seer.clone()}
                </p>
                <ul class="sd-rows">{lines}</ul>
            }
        })
        .collect_view();
    let knows = panel
        .seers
        .iter()
        .any(|(_, learned)| !learned.is_empty())
        .then(|| view! { <h2>"The seer knows"</h2> });

    let nights = (!panel.nights.is_empty()).then(|| {
        let lines = panel
            .nights
            .iter()
            .map(|night| view! { <li>{screen::night(night)}</li> })
            .collect_view();
        view! {
            <h2>"Nights"</h2>
            <ul class="sd-rows">{lines}</ul>
        }
    });

    view! {
        {decision}
        {votes}
        {tonight}
        <h2>"Players"</h2>
        <ul class="sd-players">{players}</ul>
        {knows}
        {seers}
        {nights}
    }
}
