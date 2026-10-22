//! The desktop app's main window: a start with **Play**, and **Stop**
//! while the game is prepared; then the game as it plays, with **Stop**,
//! and once it is over, the game in review from where it left off, with
//! **Save Log…** and **New game**. **File ▸ Save Game Log…** saves the log
//! too. **File ▸ Open Game Log…** reviews a log, from anywhere but a game
//! being prepared or playing, and the **Go** menu moves through either.
//! **File ▸ New Game** comes back to the start from anywhere.

use free_agent_gui::chrome::{Spinner, StartHead};
use free_agent_gui::settings::settings_get;
use free_agent_gui::style::FreeAgentStyle;
use free_agent_gui::tauri::{Channel, listen, open_url};
use free_agent_gui::theme::set_theme;
use free_agent_gui::tip::Tips;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::de::IgnoredAny;
use twenty_questions_gui_types::Cue;
use twenty_questions_gui_types::go::Go;
use twenty_questions_gui_types::settings::Snapshot;

use crate::mode::Mode;
use crate::review::Review;
use crate::tauri;
use crate::view::{Live, Reviewer, Reviewing, Viewer};

/// The app's mark: its speech bubble and question mark, without the tile.
const LOGO: &str = include_str!("../src-tauri/mark.svg");

/// The main window.
#[component]
pub fn Desktop() -> impl IntoView {
    let snapshot = RwSignal::new(None::<Snapshot>);
    let mode = RwSignal::new(Mode::Start);
    let error = RwSignal::new(None::<String>);
    let failed = RwSignal::new(None::<String>);
    let live = Live::default();
    let screen = live.screen;
    let reviewing = Reviewing::new(live);
    // Memos, so that a change of mode redraws only what it changes.
    let starting = Memo::new(move |_| mode.get().starting());
    let reviewed = Memo::new(move |_| mode.get().reviewing());
    // The channel of the game playing, kept for as long as it plays.
    let cues = StoredValue::new_local(None::<Channel<Cue>>);
    // Every cue the game has shown, to review it once it is over.
    let shown = StoredValue::new(Vec::<Cue>::new());

    spawn_local(async move {
        match settings_get().await {
            Ok(got) => snapshot.set(Some(got)),
            Err(failure) => error.set(Some(failure)),
        }
    });
    listen("settings", move |got: Snapshot| snapshot.set(Some(got)));
    Effect::new(move |_| {
        set_theme(snapshot.with(|got| {
            got.as_ref()
                .and_then(|got| got.settings.general.appearance.theme())
        }));
    });

    let run = Callback::new(move |()| {
        failed.set(None);
        mode.set(Mode::Preparing);
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
            let started = tauri::play(&channel).await;
            cues.set_value(Some(channel));
            match started {
                Ok(()) => mode.update(|mode| *mode = mode.started()),
                Err(failure) => {
                    mode.update(|mode| *mode = mode.failed());
                    failed.set(Some(failure));
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

    let start =
        move || view! { <Start snapshot=snapshot mode=mode error=error failed=failed run=run /> };
    let saveable = Memo::new(move |_| mode.get().saveable());
    let reviewer = move || {
        let controls = move || {
            if saveable.get() {
                view! {
                    <button on:click=|_| tauri::save_log() data-tip="Save the game's log, to open in review later.">"Save Log…"</button>
                    <button class="fa-primary" on:click=move |_| new_game() data-tip="Go back to the start to play another game.">"New game"</button>
                }
                .into_any()
            } else {
                view! { <button on:click=move |_| close.run(()) data-tip="Close the log and go back to the start.">"Close"</button> }.into_any()
            }
        };
        view! { <Reviewer reviewing=reviewing on_link=on_link>{controls}</Reviewer> }
    };
    view! {
        <Tips />
        <Show when=move || !reviewed.get() fallback=reviewer>
            <Show when=move || !starting.get() fallback=start>
                <Viewer live=live on_link=on_link>
                    <button on:click=|_| tauri::stop() data-tip="Stop the game here.">"Stop"</button>
                </Viewer>
            </Show>
        </Show>
    }
}

/// The start: the mark and the app's name, with a link to Settings, over
/// a line saying what the game is, a line summing up the settings, and
/// **Play**, which the settings' first error, under it, turns off. The
/// error links to its field in Settings. While the game is starting,
/// **Stop** stops it, and why it could not start shows under it.
#[component]
fn Start(
    snapshot: RwSignal<Option<Snapshot>>,
    mode: RwSignal<Mode>,
    error: RwSignal<Option<String>>,
    failed: RwSignal<Option<String>>,
    run: Callback<()>,
) -> impl IntoView {
    let summary = move || snapshot.with(|got| got.as_ref().map(|got| got.settings.summary()));
    let guesses =
        move || snapshot.with(|got| got.as_ref().map_or(0, |got| got.settings.game.guesses));
    let first_error = Memo::new(move |_| {
        snapshot.with(|got| {
            let got = got.as_ref()?;
            let error = got.settings.first_error(&got.errors)?;
            Some((error.message, error.path))
        })
    });
    let starting = move || mode.get() == Mode::Preparing;
    let blocked =
        move || starting() || snapshot.with(Option::is_none) || first_error.with(Option::is_some);
    view! {
        <FreeAgentStyle />
        <div class="fa-start fa-chrome">
            <StartHead
                logo=LOGO
                name="Twenty Questions"
                tip="Open Settings, where the players, the host and the models are chosen."
                open_settings=Callback::new(|()| tauri::settings_open(None))
            />
            <div class="fa-start-body">
                <p class="fa-tagline">"Language models put twenty yes-or-no questions to a host who is thinking of something."</p>
                {move || error.get().map(|error| view! { <pre class="fa-error">{error}</pre> })}
                <div class="tq-start">
                    <div class="tq-pips" aria-hidden="true">
                        {move || (1..=guesses()).map(|guess| view! { <span class="tq-pip">{guess}</span> }).collect_view()}
                    </div>
                    <p class="tq-summary" data-tip="The players, models and secret of the next game. Change them in Settings.">
                        {summary}
                    </p>
                    <div class="tq-run">
                        {move || starting().then(|| view! { <button class="fa-large" on:click=|_| tauri::stop() data-tip="Stop starting the game.">"Stop"</button> })}
                        <button class="tq-play fa-primary fa-large" on:click=move |_| run.run(()) disabled=blocked data-tip="Start a game with these settings. It plays live here.">
                            {move || starting().then(|| view! { <Spinner /> })}
                            "Play"
                        </button>
                    </div>
                </div>
                {move || {
                    first_error.get().map(|(message, path)| {
                        view! {
                            <p class="fa-warning tq-start-error">
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
                {move || failed.get().map(|why| view! { <p class="fa-warning tq-start-error tq-failed"><span>{why}</span></p> })}
            </div>
        </div>
    }
}
