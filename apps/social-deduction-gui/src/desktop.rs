//! The desktop app's main window: a start state listing the games, each
//! with its own **Play**, and **Stop** while the game is prepared; then
//! the game as it plays, with **Stop**, and once it is over, **New game**.
//! **File ▸ New Game** comes back to the start state from anywhere.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::RichChatStyle;
use serde::de::IgnoredAny;
use social_deduction_stage::settings::{Snapshot, game_title};
use social_deduction_stage::{GameName, Side};

use crate::screen::side_key;
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

/// The shortcut that opens Settings on the platform `platform` names, as
/// `navigator.platform` gives it.
fn settings_shortcut(platform: &str) -> &'static str {
    if platform.starts_with("Mac") {
        "⌘,"
    } else {
        "Ctrl+,"
    }
}

/// The main window.
#[component]
pub fn Desktop() -> impl IntoView {
    let snapshot = RwSignal::new(None::<Snapshot>);
    let pending = RwSignal::new(None::<GameName>);
    let error = RwSignal::new(None::<String>);
    let failed = RwSignal::new(None::<(GameName, String)>);
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

    let run = Callback::new(move |game: GameName| {
        failed.set(None);
        pending.set(Some(game));
        live.reset();
        let channel = Cues::new(move |cue| {
            watching.set(true);
            live.show(&cue);
        });
        spawn_local(async move {
            let started = tauri::play(game, &channel).await;
            cues.set_value(Some(channel));
            pending.set(None);
            if let Err(failure) = started {
                failed.set(Some((game, failure)));
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
            fallback=move || view! { <Start snapshot=snapshot pending=pending error=error failed=failed run=run /> }
        >
            <Viewer live=live on_link=on_link appearance=appearance>
                <Show
                    when=over
                    fallback=|| view! { <button on:click=|_| tauri::stop()>"Stop"</button> }
                >
                    <button class="sd-primary" on:click=move |_| new_game()>"New game"</button>
                </Show>
            </Viewer>
        </Show>
    }
}

/// The start state: the mark and the app's name, with a link to Settings,
/// over the list of games, each with its own **Play**. Why the settings
/// could not load shows over the list.
#[component]
fn Start(
    snapshot: RwSignal<Option<Snapshot>>,
    pending: RwSignal<Option<GameName>>,
    error: RwSignal<Option<String>>,
    failed: RwSignal<Option<(GameName, String)>>,
    run: Callback<GameName>,
) -> impl IntoView {
    let platform = window().navigator().platform().unwrap_or_default();
    view! {
        <RichChatStyle highlight=false fonts=false />
        <div class="sd-start">
            <header class="sd-start-head">
                <div class="sd-logo" inner_html=LOGO></div>
                <h1>"Social Deduction"</h1>
                <button class="sd-link sd-open-settings" on:click=|_| tauri::settings_open(None) data-tip="Open Settings, where the players, strategies and models of each game are chosen.">
                    "Settings " {settings_shortcut(&platform)}
                </button>
            </header>
            <div class="sd-start-body">
                <p class="sd-tagline">"Language models play games of hidden loyalty. You see every card."</p>
                {move || error.get().map(|error| view! { <pre class="sd-error">{error}</pre> })}
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
    pending: RwSignal<Option<GameName>>,
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
                {move || starting().then(|| view! { <button class="sd-large" on:click=|_| tauri::stop() data-tip="Stop starting the game.">"Stop"</button> })}
                <button class="sd-play sd-primary sd-large" on:click=move |_| run.run(game) disabled=blocked data-tip=format!("Start a game of {title} with these settings. It plays live here.")>
                    {move || starting().then(|| view! { <span class="sd-spinner"></span> })}
                    "Play " {title}
                </button>
            </div>
            {move || {
                first_error.get().map(|(message, path)| {
                    view! {
                        <p class="sd-warning sd-game-error">
                            <span>
                                {message} " "
                                <button class="sd-link" data-tip="What stops the game from starting. Open Settings at this field to fix it." on:click=move |_| tauri::settings_open(Some(path.clone()))>
                                    "Fix it in Settings"
                                </button>
                            </span>
                        </p>
                    }
                })
            }}
            {move || {
                failed.get().and_then(|(failed, why)| (failed == game).then(|| view! { <p class="sd-warning sd-game-error sd-game-failed"><span>{why}</span></p> }))
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

    #[test]
    fn settings_opens_with_command_comma_on_a_mac_and_control_comma_elsewhere() {
        assert_eq!(settings_shortcut("MacIntel"), "⌘,");
        assert_eq!(settings_shortcut("Win32"), "Ctrl+,");
        assert_eq!(settings_shortcut("Linux x86_64"), "Ctrl+,");
    }
}
