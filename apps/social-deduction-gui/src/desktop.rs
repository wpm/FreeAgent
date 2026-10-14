//! The desktop app: the settings screen until a game's first cue, then the
//! game as it plays, with **Stop**, and once it is over, **New game** and
//! **Show log file**.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::screen::Screen;
use crate::settings::Settings;
use crate::tauri::{self, Cues, Preset};
use crate::view::{Live, Viewer};

/// The desktop app.
#[component]
pub fn Desktop() -> impl IntoView {
    let presets = RwSignal::new(Vec::<Preset>::new());
    let chosen = RwSignal::new(0usize);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let live = Live::default();
    let screen = live.screen;
    let watching = RwSignal::new(false);
    // The channel of the game playing, kept for as long as it plays.
    let cues = StoredValue::new_local(None::<Cues>);

    spawn_local(async move {
        match tauri::presets().await {
            Ok(listed) => presets.set(listed),
            Err(failure) => error.set(Some(failure)),
        }
    });

    let run = Callback::new(move |()| {
        let Some(text) = presets.with(|presets| {
            presets
                .get(chosen.get_untracked())
                .map(|preset| preset.text.clone())
        }) else {
            return;
        };
        error.set(None);
        pending.set(true);
        screen.set(Screen::default());
        let channel = Cues::new(move |cue| {
            watching.set(true);
            live.show(&cue);
        });
        spawn_local(async move {
            let started = tauri::play(&text, &channel).await;
            cues.set_value(Some(channel));
            pending.set(false);
            if let Err(failure) = started {
                error.set(Some(failure));
            }
        });
    });

    let over = move || screen.with(|screen| screen.outcome.is_some());
    let new_game = move |_| {
        watching.set(false);
        screen.set(Screen::default());
        // The settings screen follows the system's theme, not the game's.
        if let Some(root) = document().document_element() {
            let _ = root.remove_attribute("data-theme");
        }
    };

    // A link opens in the system browser, since the window must not
    // navigate away from the game.
    let on_link = Callback::new(tauri::open_url);

    view! {
        <Show
            when=move || watching.get()
            fallback=move || {
                view! {
                    <Settings presets=presets chosen=chosen pending=pending error=error run=run />
                }
            }
        >
            <Viewer live=live on_link=on_link>
                <Show
                    when=over
                    fallback=|| view! { <button on:click=|_| tauri::stop()>"Stop"</button> }
                >
                    <button on:click=new_game>"New game"</button>
                    <button on:click=|_| tauri::show_log()>"Show log file"</button>
                </Show>
            </Viewer>
        </Show>
    }
}
