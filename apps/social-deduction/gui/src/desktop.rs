//! The desktop app's main window: a start state listing the games, each
//! with its own **Play**, and **Stop** while the game is prepared; then
//! the game as it plays, with **Stop**, and once it is over, the game in
//! review from where it left off, with **Save Log…** and **New game**.
//! **File ▸ Save Game Log…** saves the log too.
//! **File ▸ Open Game Log…** reviews a log, from anywhere but a game being
//! prepared or playing, and the **Go** menu moves through either. **File
//! ▸ New Game** comes back to the start state from anywhere.

use free_agent_gui::chrome::{Spinner, StartHead};
use free_agent_gui::style::FreeAgentStyle;
use free_agent_gui::tauri::{Channel, listen, open_url};
use free_agent_gui::theme::set_theme;
use free_agent_gui::tip::Tips;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::de::IgnoredAny;
use social_deduction_gui_types::go::Go;
use social_deduction_gui_types::settings::{Snapshot, game_title};
use social_deduction_gui_types::{Cue, GameName, Side};

use crate::mode::Mode;
use crate::review::Review;
use crate::screen::side_key;
use crate::tauri;
use crate::view::{Live, Reviewer, Reviewing, Viewer};

/// The app's mark: its two balloons, without the tile.
const LOGO: &str = include_str!("../src-tauri/mark.svg");

/// The main window.
#[component]
pub fn Desktop() -> impl IntoView {
    let snapshot = RwSignal::new(None::<Snapshot>);
    let mode = RwSignal::new(Mode::Start);
    let error = RwSignal::new(None::<String>);
    let failed = RwSignal::new(None::<(GameName, String)>);
    let live = Live::default();
    let screen = live.screen;
    let reviewing = Reviewing::new(live);
    // Memos, so that a change of mode redraws only what it changes.
    let listing = Memo::new(move |_| mode.get().listing());
    let reviewed = Memo::new(move |_| mode.get().reviewing());
    let pending = Memo::new(move |_| mode.get().preparing());
    // The channel of the game playing, kept for as long as it plays.
    let cues = StoredValue::new_local(None::<Channel<Cue>>);
    // Every cue the game has shown, to review it once it is over.
    let shown = StoredValue::new(Vec::<Cue>::new());

    spawn_local(async move {
        match tauri::settings_get().await {
            Ok(got) => snapshot.set(Some(got)),
            Err(failure) => error.set(Some(failure)),
        }
    });
    listen("settings", move |got: Snapshot| snapshot.set(Some(got)));

    let appearance = Signal::derive(move || {
        snapshot.with(|got| {
            got.as_ref()
                .and_then(|got| got.settings.general.appearance.theme())
        })
    });
    Effect::new(move |_| {
        if listing.get() {
            set_theme(appearance.get());
        }
    });

    let run = Callback::new(move |game: GameName| {
        failed.set(None);
        mode.set(Mode::Preparing(game));
        live.reset();
        shown.set_value(Vec::new());
        let channel = Channel::new(move |cue: Cue| {
            let now = mode.get_untracked();
            if !now.busy() {
                return;
            }
            live.show(&cue);
            shown.update_value(|shown| shown.push(cue));
            let over = screen.with_untracked(|screen| screen.outcome.is_some());
            if let Some(next) = now.cued(over).filter(|&next| next != now) {
                // The cues go to the review, which has no more need of them.
                if over
                    && let Some(review) =
                        Review::played(&shown.try_update_value(std::mem::take).unwrap_or_default())
                {
                    reviewing.open(review);
                }
                mode.set(next);
            }
        });
        spawn_local(async move {
            let started = tauri::play(game, &channel).await;
            cues.set_value(Some(channel));
            match started {
                Ok(()) => mode.update(|mode| *mode = mode.started()),
                Err(failure) => {
                    mode.update(|mode| *mode = mode.failed());
                    failed.set(Some((game, failure)));
                }
            }
        });
    });

    let new_game = move || {
        let now = mode.get_untracked();
        if now.reviewing() {
            reviewing.close();
        } else if now.busy() {
            tauri::stop();
        }
        mode.set(now.new_game());
        live.reset();
    };
    // Back from a log to the start.
    let close = Callback::new(move |()| {
        reviewing.close();
        mode.set(Mode::Start);
    });
    listen("new-game", move |_: IgnoredAny| new_game());
    listen("review", move |opened| match Review::new(opened) {
        Ok(review) => {
            reviewing.open(review);
            mode.set(Mode::ReviewingLog);
        }
        Err(failure) => error.set(Some(failure)),
    });
    listen("go", move |go: Go| reviewing.go(go));
    // A memo, so that moving the cursor leaves the menus be.
    let menus = Memo::new(move |_| {
        let games = reviewing
            .review
            .with(|review| review.as_ref().map_or(0, Review::games));
        let mode = mode.get();
        (!mode.busy(), mode.saveable(), games)
    });
    Effect::new(move |_| {
        let (open, save, games) = menus.get();
        tauri::menus(open, save, games);
    });

    // A link opens in the system browser, since the window must not
    // navigate away from the game.
    let on_link = Callback::new(open_url);

    let start = move || view! { <Start snapshot=snapshot pending=pending error=error failed=failed run=run /> };
    let saveable = Memo::new(move |_| mode.get().saveable());
    let reviewer = move || {
        let controls = move || {
            if saveable.get() {
                view! {
                    <button on:click=|_| tauri::save_log() data-tip="Save the game's log, to open in review later.">"Save Log…"</button>
                    <button class="fa-primary" on:click=move |_| new_game()>"New game"</button>
                }
                .into_any()
            } else {
                view! { <button on:click=move |_| close.run(()) data-tip="Close the log and go back to the games.">"Close"</button> }.into_any()
            }
        };
        view! {
            <Reviewer reviewing=reviewing on_link=on_link appearance=appearance>
                {controls}
            </Reviewer>
        }
    };
    view! {
        <Tips />
        <Show
            when=move || !reviewed.get()
            fallback=reviewer
        >
        <Show
            when=move || !listing.get()
            fallback=start
        >
            <Viewer live=live on_link=on_link appearance=appearance>
                <button on:click=|_| tauri::stop()>"Stop"</button>
            </Viewer>
        </Show>
        </Show>
    }
}

/// The start state: the mark and the app's name, with a link to Settings,
/// over the list of games, each with its own **Play**. Why the settings
/// could not load shows over the list.
#[component]
fn Start(
    snapshot: RwSignal<Option<Snapshot>>,
    pending: Memo<Option<GameName>>,
    error: RwSignal<Option<String>>,
    failed: RwSignal<Option<(GameName, String)>>,
    run: Callback<GameName>,
) -> impl IntoView {
    view! {
        <FreeAgentStyle />
        <div class="fa-start fa-chrome">
            <StartHead
                logo=LOGO
                name="Social Deduction"
                tip="Open Settings, where the players, strategies and models of each game are chosen."
                open_settings=Callback::new(|()| tauri::settings_open(None))
            />
            <div class="fa-start-body">
                <p class="fa-tagline">"Language models play games of hidden loyalty. You see every card."</p>
                {move || error.get().map(|error| view! { <pre class="fa-error">{error}</pre> })}
                <ul class="sd-game-list">
                    {GameName::ALL
                        .into_iter()
                        .map(|game| view! { <Game game=game snapshot=snapshot pending=pending failed=failed run=run /> })
                        .collect_view()}
                </ul>
            </div>
        </div>
    }
}

/// One game in the list: its name, what it is, its seats and a line
/// summing up its settings, with its **Play**, which the game's first
/// error, under the row, disables. The error links to its field in
/// Settings. While the game is starting, **Stop** stops it, and why it
/// could not start shows under the row.
#[component]
fn Game(
    game: GameName,
    snapshot: RwSignal<Option<Snapshot>>,
    pending: Memo<Option<GameName>>,
    failed: RwSignal<Option<(GameName, String)>>,
    run: Callback<GameName>,
) -> impl IntoView {
    let title = game_title(game);
    let seats = Memo::new(move |_| {
        snapshot.with(|got| {
            got.as_ref()
                .map(|got| got.settings.game(game).seats())
                .unwrap_or_default()
        })
    });
    let summary = move || snapshot.with(|got| got.as_ref().map(|got| got.settings.summary(game)));
    let first_error = Memo::new(move |_| {
        snapshot.with(|got| {
            let got = got.as_ref()?;
            let error = got.settings.errors_of(game, &got.errors).next()?;
            Some((error.message.clone(), error.path.clone()))
        })
    });
    let starting = move || pending.get() == Some(game);
    let blocked = move || {
        pending.with(Option::is_some)
            || snapshot.with(Option::is_none)
            || first_error.with(Option::is_some)
    };
    view! {
        <li class="sd-game">
            <div class="sd-game-about">
                <h2>{title}</h2>
                <p class="sd-description">{game.description()}</p>
                <div class="sd-game-table" data-tip="The players, strategies and models of the next game. Change them in Settings.">
                    <span class="sd-game-seats" role="img" aria-label=move || seats.with(|seats| said(seats))>
                        {move || {
                            seats
                                .get()
                                .into_iter()
                                .map(|side| view! { <span class=format!("sd-{}", side_key(side))></span> })
                                .collect_view()
                        }}
                    </span>
                    <span class="sd-game-summary">{summary}</span>
                </div>
            </div>
            <div class="sd-run">
                {move || starting().then(|| view! { <button class="fa-large" on:click=|_| tauri::stop() data-tip="Stop starting the game.">"Stop"</button> })}
                <button class="sd-play fa-primary fa-large" on:click=move |_| run.run(game) disabled=blocked data-tip=format!("Start a game of {title} with these settings. It plays live here, and you can save its log when it ends.")>
                    {move || starting().then(|| view! { <Spinner /> })}
                    "Play " {title}
                </button>
            </div>
            {move || {
                first_error.get().map(|(message, path)| {
                    view! {
                        <p class="fa-warning sd-game-error">
                            <span>
                                {message} " "
                                <button class="fa-link" data-tip="What stops the game from starting. Open Settings at this field to fix it." on:click=move |_| tauri::settings_open(Some(path.clone()))>
                                    "Fix it in Settings"
                                </button>
                            </span>
                        </p>
                    }
                })
            }}
            {move || {
                failed.get().and_then(|(failed, why)| (failed == game).then(|| view! { <p class="fa-warning sd-game-error sd-game-failed"><span>{why}</span></p> }))
            }}
        </li>
    }
}

/// What `seats` holds, as a screen reader says it: `2 evil, 5 good`.
fn said(seats: &[Side]) -> String {
    let evil = seats.iter().filter(|side| **side == Side::Evil).count();
    format!("{evil} evil, {} good", seats.len() - evil)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seats_are_said_as_how_many_of_each_side() {
        assert_eq!(
            said(&[Side::Evil, Side::Good, Side::Good]),
            "1 evil, 2 good"
        );
        assert_eq!(said(&[]), "0 evil, 0 good");
    }
}
