//! The desktop app's main window: a start state, where a game is chosen
//! and **Play** pressed, with **Stop** while the game is prepared; then
//! the game as it plays, with **Stop**, and once it is over, **New game**.
//! **File ▸ New Game** comes back to the start state from anywhere.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::RichChatStyle;
use serde::de::IgnoredAny;
use social_deduction_stage::GameName;
use social_deduction_stage::settings::{Snapshot, game_title};

use crate::tauri::{self, Cues};
use crate::tip::Tips;
use crate::view::{Live, Viewer};

/// The app's mark: its two balloons, without the tile.
const LOGO: &str = include_str!("../src-tauri/mark.svg");

/// Hold the page to `theme`, or leave it to the system.
pub(crate) fn set_theme(theme: Option<&str>) {
    if let Some(root) = document().document_element() {
        let _ = match theme {
            Some(theme) => root.set_attribute("data-theme", theme),
            None => root.remove_attribute("data-theme"),
        };
    }
}

/// The game the settings in `snapshot` choose, Werewolf until they come.
fn chosen(snapshot: RwSignal<Option<Snapshot>>) -> Signal<GameName> {
    Signal::derive(move || {
        snapshot.with(|got| {
            got.as_ref()
                .map_or(GameName::Werewolf, |got| got.settings.general.game)
        })
    })
}

/// The main window.
#[component]
pub fn Desktop() -> impl IntoView {
    let snapshot = RwSignal::new(None::<Snapshot>);
    let game = chosen(snapshot);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let live = Live::default();
    let screen = live.screen;
    let watching = RwSignal::new(false);
    // The channel of the game playing, kept for as long as it plays.
    let cues = StoredValue::new_local(None::<Cues>);

    spawn_local(async move {
        match tauri::settings_get().await {
            Ok(got) => snapshot.set(Some(got)),
            Err(failure) => error.set(Some(failure)),
        }
    });
    tauri::listen("settings", move |got: Snapshot| snapshot.set(Some(got)));

    let appearance = Signal::derive(move || {
        snapshot.with(|got| {
            got.as_ref()
                .and_then(|got| got.settings.general.appearance.theme())
        })
    });
    Effect::new(move |_| {
        if !watching.get() {
            set_theme(appearance.get());
        }
    });

    let run = Callback::new(move |()| {
        error.set(None);
        pending.set(true);
        live.reset();
        let channel = Cues::new(move |cue| {
            watching.set(true);
            live.show(&cue);
        });
        let chosen = game.get_untracked();
        spawn_local(async move {
            let started = tauri::play(chosen, &channel).await;
            cues.set_value(Some(channel));
            pending.set(false);
            if let Err(failure) = started {
                error.set(Some(failure));
            }
        });
    });

    let over = move || screen.with(|screen| screen.outcome.is_some());
    let new_game = move || {
        if !over() {
            tauri::stop();
        }
        watching.set(false);
        live.reset();
    };
    tauri::listen("new-game", move |_: IgnoredAny| new_game());

    // A link opens in the system browser, since the window must not
    // navigate away from the game.
    let on_link = Callback::new(tauri::open_url);

    view! {
        <Tips />
        <Show
            when=move || watching.get()
            fallback=move || view! { <Start snapshot=snapshot pending=pending error=error run=run /> }
        >
            <Viewer live=live on_link=on_link appearance=appearance>
                <Show
                    when=over
                    fallback=|| view! { <button on:click=|_| tauri::stop()>"Stop"</button> }
                >
                    <button on:click=move |_| new_game()>"New game"</button>
                </Show>
            </Viewer>
        </Show>
    }
}

/// The start state: the logo, the game to play, one line saying what it
/// is, and **Play**, which the game's first error, under it, disables. The
/// error links to its field in Settings. While a game is starting,
/// **Stop** stops it, and why one could not start shows under **Play**.
#[component]
fn Start(
    snapshot: RwSignal<Option<Snapshot>>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    run: Callback<()>,
) -> impl IntoView {
    let game = chosen(snapshot);
    let summary =
        move || snapshot.with(|got| got.as_ref().map(|got| got.settings.summary(game.get())));
    let first_error = Memo::new(move |_| {
        snapshot.with(|got| {
            let got = got.as_ref()?;
            let error = got.settings.first_error(game.get(), &got.errors)?;
            Some((error.message, error.path))
        })
    });
    let blocked = move || {
        pending.get() || snapshot.with(Option::is_none) || first_error.with(Option::is_some)
    };
    let choice = move |choice: GameName| {
        view! {
            <button
                data-tip=format!("Play {0} next. Its settings are on the {0} tab of Settings.", game_title(choice))
                class:sd-segment-on=move || game.get() == choice
                aria-pressed=move || (game.get() == choice).to_string()
                on:click=move |_| {
                    snapshot.update(|got| {
                        if let Some(got) = got {
                            got.settings.general.game = choice;
                        }
                    });
                    tauri::game_choose(choice);
                }
            >
                {game_title(choice)}
            </button>
        }
    };
    view! {
        <RichChatStyle highlight=false fonts=false />
        <div class="sd-start">
            <div class="sd-logo" inner_html=LOGO></div>
            <div class="sd-segments" role="group" aria-label="Game">
                {choice(GameName::Werewolf)}
                {choice(GameName::Avalon)}
            </div>
            <p class="sd-summary" data-tip="The players, strategies and models of the next game. Change them in Settings.">{summary}</p>
            <div class="sd-run">
                <button class="sd-play" on:click=move |_| run.run(()) disabled=blocked data-tip="Start a game with these settings. It plays live here.">
                    {move || pending.get().then(|| view! { <span class="sd-spinner"></span> })}
                    "Play"
                </button>
                {move || pending.get().then(|| view! { <button on:click=|_| tauri::stop() data-tip="Stop starting the game.">"Stop"</button> })}
            </div>
            {move || {
                first_error.get().map(|(message, path)| {
                    view! {
                        <button class="sd-link sd-first-error" data-tip="What stops the game from starting. Open Settings at this field to fix it." on:click=move |_| tauri::settings_open(Some(path.clone()))>
                            {message}
                        </button>
                    }
                })
            }}
            {move || error.get().map(|error| view! { <pre class="sd-error">{error}</pre> })}
        </div>
    }
}
