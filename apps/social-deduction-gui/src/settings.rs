//! The desktop app's Settings window: General, Models, Werewolf and
//! Avalon tabs, with every change stored the moment it is made and each
//! field's error in red beneath it.
//!
//! The window holds a copy of the [`AppSettings`]. Each change is made to
//! it and sent to the backend whole, which stores it and answers with what
//! is wrong. Settings changed elsewhere, by an import, come back as a
//! `settings` event, and a `focus` event, or the hash it opens at, names a
//! field to show.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::RichChatStyle;
use social_deduction_stage::GameName;
use social_deduction_stage::settings::{
    AppSettings, Appearance, FieldError, GameSettings, ModelSettings, RoleSettings, Snapshot,
    StrategySettings, game_key, game_title,
};
use wasm_bindgen::JsCast;

use crate::desktop::set_theme;
use crate::tauri;
use crate::tip::Tips;

mod game;
mod models;

use game::GameTab;
use models::Models;

/// The tabs, in order.
const TABS: [Tab; 4] = [
    Tab::General,
    Tab::Models,
    Tab::Game(GameName::Werewolf),
    Tab::Game(GameName::Avalon),
];

/// A tab of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The look.
    General,
    /// The shared models and their providers.
    Models,
    /// One game's settings.
    Game(GameName),
}

impl Tab {
    /// What the tab is called.
    pub fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Models => "Models",
            Tab::Game(game) => game_title(game),
        }
    }

    /// What the tab holds, as its help says.
    pub fn tip(self) -> &'static str {
        match self {
            Tab::General => "The app's appearance.",
            Tab::Models => "Model providers, their keys, and the models strategies can use.",
            Tab::Game(GameName::Werewolf) => {
                "Werewolf's players, phase limits, strategies and prompt parts."
            }
            Tab::Game(GameName::Avalon) => {
                "Avalon's players, phase limits, strategies and prompt parts."
            }
        }
    }

    /// The tab a field's `path` is on.
    pub fn of(path: &[String]) -> Tab {
        match path.first().map(String::as_str) {
            Some("models") => Tab::Models,
            Some(key) if key == game_key(GameName::Werewolf) => Tab::Game(GameName::Werewolf),
            Some(key) if key == game_key(GameName::Avalon) => Tab::Game(GameName::Avalon),
            _ => Tab::General,
        }
    }
}

/// The field a hash names: `#settings=werewolf/strategies/everyone`, its
/// segments as `decode` decodes each, or none.
pub fn hashed(hash: &str, decode: impl Fn(&str) -> String) -> Option<Vec<String>> {
    let (_, path) = hash.split_once("#settings=")?;
    Some(path.split('/').map(decode).collect())
}

/// The id of the element of the field at `path`.
pub fn field_id(path: &[String]) -> String {
    format!("field-{}", path.join("."))
}

/// `path` as owned strings.
fn path(path: &[&str]) -> Vec<String> {
    path.iter().map(ToString::to_string).collect()
}

/// The message of the error at `path` among `errors`, if there is one.
pub fn error_at(errors: &[FieldError], path: &[String]) -> Option<String> {
    errors
        .iter()
        .find(|error| error.is_at(path))
        .map(|error| error.message.clone())
}

/// The settings as the window holds them, and what is wrong with them.
#[derive(Clone, Copy)]
struct Editor {
    settings: RwSignal<AppSettings>,
    errors: RwSignal<Vec<FieldError>>,
    sent: StoredValue<u64>,
}

impl Editor {
    /// Change the settings with `change`, and send them to be stored.
    fn edit(self, change: impl FnOnce(&mut AppSettings)) {
        self.settings.update(change);
        self.send();
    }

    /// Send the settings to be stored, and keep what is wrong with them
    /// unless a later change has been sent since.
    fn send(self) {
        let number = self.sent.get_value() + 1;
        self.sent.set_value(number);
        let settings = self.settings.get_untracked();
        spawn_local(async move {
            match tauri::settings_set(&settings).await {
                Ok(errors) if self.sent.get_value() == number => self.errors.set(errors),
                Ok(_) => {}
                Err(failure) => web_sys::console::error_1(&failure.into()),
            }
        });
    }

    /// Change the settings of `game` with `change`.
    fn edit_game(self, game: GameName, change: impl FnOnce(&mut GameSettings)) {
        self.edit(|settings| change(settings.game_mut(game)));
    }

    /// Change the role `role` of `game` with `change`.
    fn edit_role(self, game: GameName, role: &str, change: impl FnOnce(&mut RoleSettings)) {
        self.edit_game(game, |settings| {
            if let Some(found) = settings.roles.iter_mut().find(|r| r.key == role) {
                change(found);
            }
        });
    }

    /// Change the strategy `name` of `game` with `change`.
    fn edit_strategy(self, game: GameName, name: &str, change: impl FnOnce(&mut StrategySettings)) {
        self.edit_game(game, |settings| {
            if let Some(strategy) = settings.strategies.get_mut(name) {
                change(strategy);
            }
        });
    }

    /// Change the model `name` with `change`.
    fn edit_model(self, name: &str, change: impl FnOnce(&mut ModelSettings)) {
        self.edit(|settings| {
            if let Some(model) = settings.models.get_mut(name) {
                change(model);
            }
        });
    }

    /// The error at `path`, telling of a change only when it changes.
    fn error(self, at: Vec<String>) -> Signal<Option<String>> {
        let errors = self.errors;
        Memo::new(move |_| errors.with(|errors| error_at(errors, &at))).into()
    }

    /// The settings of `game`, read with `read`, telling of a change only
    /// when what is read changes.
    fn game<T: Clone + PartialEq + Send + Sync + 'static>(
        self,
        game: GameName,
        read: impl Fn(&GameSettings) -> T + Send + Sync + 'static,
    ) -> Memo<T> {
        let settings = self.settings;
        Memo::new(move |_| settings.with(|settings| read(settings.game(game))))
    }
}

/// The Settings window.
#[component]
pub fn SettingsWindow() -> impl IntoView {
    let editor = Editor {
        settings: RwSignal::new(AppSettings::default()),
        errors: RwSignal::new(Vec::new()),
        sent: StoredValue::new(0),
    };
    let loaded = RwSignal::new(false);
    let opened = hashed(&window().location().hash().unwrap_or_default(), |segment| {
        js_sys::decode_uri_component(segment)
            .ok()
            .and_then(|decoded| decoded.as_string())
            .unwrap_or_else(|| segment.to_string())
    });
    let tab = RwSignal::new(opened.as_deref().map_or(Tab::General, Tab::of));
    let focus = RwSignal::new(opened);
    let take = move |got: Snapshot| {
        editor.settings.set(got.settings);
        editor.errors.set(got.errors);
        loaded.set(true);
    };
    spawn_local(async move {
        match tauri::settings_get().await {
            Ok(got) => take(got),
            Err(failure) => web_sys::console::error_1(&failure.into()),
        }
    });
    tauri::listen("settings", take);
    tauri::listen("focus", move |at: Vec<String>| {
        tab.set(Tab::of(&at));
        focus.set(Some(at));
    });
    // Once the field to show is drawn, scroll to it and focus it.
    Effect::new(move |_| {
        let (Some(at), true) = (focus.get(), loaded.get()) else {
            return;
        };
        let id = field_id(&at);
        set_timeout(
            move || {
                if let Some(element) = document().get_element_by_id(&id) {
                    element.scroll_into_view();
                    if let Ok(element) = element.dyn_into::<web_sys::HtmlElement>() {
                        let _ = element.focus();
                    }
                }
            },
            std::time::Duration::from_millis(50),
        );
    });
    Effect::new(move |_| {
        set_theme(
            editor
                .settings
                .with(|settings| settings.general.appearance.theme()),
        );
    });

    view! {
        <RichChatStyle highlight=false fonts=false />
        <Tips />
        <div class="sd-settings-window">
            <nav class="sd-tabs" role="tablist">
                {TABS
                    .into_iter()
                    .map(|each| {
                        view! {
                            <button
                                role="tab"
                                data-tip=each.tip()
                                class:sd-tab-on=move || tab.get() == each
                                aria-selected=move || (tab.get() == each).to_string()
                                on:click=move |_| tab.set(each)
                            >
                                {each.title()}
                            </button>
                        }
                    })
                    .collect_view()}
            </nav>
            <main class="sd-settings">
                // Nothing is changed before the settings have come, so
                // that no change overwrites them.
                {move || match (loaded.get(), tab.get()) {
                    (false, _) => ().into_any(),
                    (true, Tab::General) => view! { <General editor=editor /> }.into_any(),
                    (true, Tab::Models) => view! { <Models editor=editor /> }.into_any(),
                    (true, Tab::Game(game)) => view! { <GameTab editor=editor game=game /> }.into_any(),
                }}
            </main>
        </div>
    }
}

/// An error beneath a field, in red.
#[component]
fn Problem(error: Signal<Option<String>>) -> impl IntoView {
    move || {
        error
            .get()
            .map(|error| view! { <p class="sd-field-error">{error}</p> })
    }
}

/// The General tab: the look.
#[component]
fn General(editor: Editor) -> impl IntoView {
    let appearance = move |choice: Appearance, title: &'static str, tip: &'static str| {
        let chosen = move || editor.settings.with(|s| s.general.appearance == choice);
        view! {
            <button
                data-tip=tip
                class:sd-segment-on=chosen
                aria-pressed=move || chosen().to_string()
                on:click=move |_| editor.edit(|s| s.general.appearance = choice)
            >
                {title}
            </button>
        }
    };
    view! {
        <section class="sd-section">
            <h2 data-tip="How the app looks.">"Appearance"</h2>
            <div class="sd-segments" role="group" aria-label="Appearance">
                {appearance(Appearance::System, "System", "Follow the system's look. Werewolf turns dark by night and light by day.")}
                {appearance(Appearance::Light, "Light", "Always light, whatever the system or the game shows.")}
                {appearance(Appearance::Dark, "Dark", "Always dark, whatever the system or the game shows.")}
            </div>
            <p class="sd-note">"System follows the system's look, and keeps Werewolf dark by night and light by day."</p>
        </section>
    }
}

/// A select among `options`, each a value and its label, over `value`,
/// which `set` changes, with the error at `at` beneath it. Without a
/// `label`, the select is named by its `tip`.
#[component]
fn Choice(
    #[prop(optional, into)] label: Option<String>,
    value: Signal<String>,
    options: Signal<Vec<(String, String)>>,
    set: Callback<String>,
    editor: Editor,
    at: Vec<String>,
    #[prop(optional, into)] tip: Option<String>,
    #[prop(optional, into)] label_tip: Option<String>,
) -> impl IntoView {
    let error = editor.error(at.clone());
    let choices = move || {
        let chosen = value.get();
        let mut options = options.get();
        // A value held is shown even when it is not an option.
        if !options.iter().any(|(value, _)| *value == chosen) {
            options.push((chosen.clone(), chosen.clone()));
        }
        options
            .into_iter()
            .map(|(value, label)| {
                let selected = value == chosen;
                view! { <option value=value selected=selected>{label}</option> }
            })
            .collect_view()
    };
    let named = label.is_none().then(|| tip.clone()).flatten();
    let shown = label.map(|label| view! { <span data-tip=label_tip>{label}</span> });
    view! {
        <label class="sd-field">
            {shown}
            <select
                class="sd-select"
                id=field_id(&at)
                aria-label=named
                data-tip=tip
                class:sd-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:change=move |event| set.run(event_target_value(&event))
            >
                {choices}
            </select>
        </label>
        <Problem error=error />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_s_tab_is_the_first_part_of_its_path() {
        assert_eq!(Tab::of(&path(&["models", "gpt", "id"])), Tab::Models);
        assert_eq!(
            Tab::of(&path(&["werewolf", "roles"])),
            Tab::Game(GameName::Werewolf)
        );
        assert_eq!(Tab::of(&path(&["avalon"])), Tab::Game(GameName::Avalon));
        assert_eq!(Tab::of(&path(&["general", "appearance"])), Tab::General);
        assert_eq!(Tab::of(&[]), Tab::General);
        let titles: Vec<&str> = TABS.iter().map(|tab| tab.title()).collect();
        assert_eq!(titles, ["General", "Models", "Werewolf", "Avalon"]);
    }

    #[test]
    fn the_hash_names_the_field_to_show() {
        let decode = |segment: &str| segment.replace("%20", " ");
        assert_eq!(
            hashed("#settings=werewolf/strategies/my%20one/model", decode),
            Some(path(&["werewolf", "strategies", "my one", "model"]))
        );
        assert_eq!(hashed("#settings", decode), None);
        assert_eq!(
            field_id(&path(&["models", "gpt", "id"])),
            "field-models.gpt.id"
        );
    }

    #[test]
    fn an_error_is_found_at_its_path() {
        let errors = [FieldError::new(&["models", "gpt", "id"], "needs an id")];
        assert_eq!(
            error_at(&errors, &path(&["models", "gpt", "id"])).as_deref(),
            Some("needs an id")
        );
        assert_eq!(error_at(&errors, &path(&["models", "gpt"])), None);
    }
}
