//! The page: a header, the chat beside the board, and a footer naming the
//! commit the page was built from. Every part draws the [`Screen`].

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::{Message, RenderOptions, RichChatStyle, RichText, Transcript};
use social_deduction_stage::{Cue, Deed, GameName, Outcome, Side};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::screen::{
    Board, Body, Landed, NARRATOR, Post, Screen, THINKING_DELAY, Thinker, dots_due, entry, role_key,
};

/// How often a replay shows the next cue.
const STEP: Duration = Duration::from_millis(250);

/// How long a choice flashes as it lands.
const FLASH: Duration = Duration::from_millis(900);

/// The commit the page was built from, when the build names one.
const COMMIT: Option<&str> = option_env!("GITHUB_SHA");

/// The viewer: it fetches `cues.json` from beside the page and shows the
/// whole game at once. **Play** clears the screen and replays the cues,
/// one every 250 ms.
#[component]
pub fn App() -> impl IntoView {
    let screen = RwSignal::new(Screen::default());
    let cues = StoredValue::new(Vec::<Cue>::new());
    let failure = RwSignal::new(None::<String>);
    let replayed = RwSignal::new(false);
    let replay = StoredValue::new(None::<IntervalHandle>);

    spawn_local(async move {
        match fetched().await {
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

    let play = move |_| {
        if let Some(handle) = replay.get_value() {
            handle.clear();
        }
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
                if let Some(Some(number)) = screen.try_update(|screen| screen.apply(&cue, true)) {
                    set_timeout(move || screen.update(|screen| screen.expire(number)), FLASH);
                }
                if let Some(due) = dots_due(&cue) {
                    let rate = screen.with_untracked(|screen| screen.rate);
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
                <p class="sd-phase">{move || screen.with(|screen| screen.heading.clone())}</p>
                {move || screen.with(|screen| screen.outcome.clone()).map(outcome)}
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
                </aside>
            </main>
            <footer class="sd-footer">{footer()}</footer>
        </div>
    }
}

/// The cues in `cues.json`, beside the page.
async fn fetched() -> Result<Vec<Cue>, String> {
    let failed = |error: JsValue| format!("cues.json: {error:?}");
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str("cues.json"))
        .await
        .map_err(failed)?
        .dyn_into()
        .map_err(failed)?;
    if !response.ok() {
        return Err(format!("cues.json: {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(failed)?)
        .await
        .map_err(failed)?;
    serde_json::from_str(&text.as_string().unwrap_or_default())
        .map_err(|error| format!("cues.json: {error}"))
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
fn chooser(board: &Board, player: &str) -> String {
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
fn swatch(board: &Board, player: &str) -> String {
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
fn flash(board: &Board, choice: &Landed, player: &str) -> (bool, String) {
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
            let class = if by_deed || by_team {
                "sd-chip sd-landed"
            } else {
                "sd-chip"
            };
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
                    let class = if landing { "sd-landed" } else { "" };
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
        let class = if landing {
            "sd-assassination sd-landed"
        } else {
            "sd-assassination"
        };
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
