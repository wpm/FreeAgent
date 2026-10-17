//! The desktop app's Settings window: a sidebar of pages, General, Models
//! and one per game, each page a few grouped lists, with every change
//! stored the moment it is made and each field's error beneath it.
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

/// The pages, in order: General, Models, then one per game.
fn tabs() -> Vec<Tab> {
    [Tab::General, Tab::Models]
        .into_iter()
        .chain(GameName::ALL.map(Tab::Game))
        .collect()
}

/// A page of the window, a tab of its sidebar.
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
    pub fn tip(self) -> String {
        match self {
            Tab::General => "The app's appearance.".to_string(),
            Tab::Models => {
                "Model providers, their keys, and the models strategies can use.".to_string()
            }
            Tab::Game(game) => format!(
                "{}'s players, phase limits, strategies and prompt parts.",
                game_title(game)
            ),
        }
    }

    /// The id of the tab's button in the sidebar.
    fn id(self) -> String {
        match self {
            Tab::General => "tab-general".to_string(),
            Tab::Models => "tab-models".to_string(),
            Tab::Game(game) => format!("tab-{}", game_key(game)),
        }
    }

    /// The tab a field's `path` is on.
    pub fn of(path: &[String]) -> Tab {
        match path.first().map(String::as_str) {
            Some("models") => Tab::Models,
            Some(key) => GameName::ALL
                .into_iter()
                .find(|&game| game_key(game) == key)
                .map_or(Tab::General, Tab::Game),
            None => Tab::General,
        }
    }

    /// The tab the key `key` moves to from this one, if it moves any: an
    /// arrow to the next or the previous, round the ends, and Home and End
    /// to the first and the last.
    pub fn moved(self, key: &str) -> Option<Tab> {
        let tabs = tabs();
        let at = tabs.iter().position(|&tab| tab == self)?;
        let to = match key {
            "ArrowDown" | "ArrowRight" => (at + 1) % tabs.len(),
            "ArrowUp" | "ArrowLeft" => (at + tabs.len() - 1) % tabs.len(),
            "Home" => 0,
            "End" => tabs.len() - 1,
            _ => return None,
        };
        Some(tabs[to])
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

    // The arrows, Home and End move between the pages, and the focus
    // goes with them.
    let step = move |event: web_sys::KeyboardEvent| {
        let Some(to) = tab.get_untracked().moved(&event.key()) else {
            return;
        };
        event.prevent_default();
        tab.set(to);
        if let Some(button) = document()
            .get_element_by_id(&to.id())
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = button.focus();
        }
    };

    let button = move |each: Tab| {
        let on = move || tab.get() == each;
        view! {
            <button
                role="tab"
                id=each.id()
                data-tip=each.tip()
                aria-selected=move || on().to_string()
                tabindex=move || if on() { "0" } else { "-1" }
                on:click=move |_| tab.set(each)
            >
                {each.title()}
            </button>
        }
    };

    view! {
        <RichChatStyle highlight=false fonts=false />
        <Tips />
        <div class="sd-settings-window">
            <nav class="sd-sidebar" role="tablist" aria-orientation="vertical" aria-label="Settings" on:keydown=step>
                {button(Tab::General)}
                {button(Tab::Models)}
                <div class="sd-sidebar-label" role="presentation">"Games"</div>
                {GameName::ALL.into_iter().map(|game| button(Tab::Game(game))).collect_view()}
            </nav>
            <main class="sd-settings">
                <div class="sd-page">
                    // Nothing is changed before the settings have come, so
                    // that no change overwrites them.
                    {move || match (loaded.get(), tab.get()) {
                        (false, _) => ().into_any(),
                        (true, Tab::General) => view! { <General editor=editor /> }.into_any(),
                        (true, Tab::Models) => view! { <Models editor=editor /> }.into_any(),
                        (true, Tab::Game(game)) => view! { <GameTab editor=editor game=game /> }.into_any(),
                    }}
                </div>
            </main>
        </div>
    }
}

/// An error beneath a field, flagged in marigold.
#[component]
fn Problem(error: Signal<Option<String>>) -> impl IntoView {
    move || {
        error
            .get()
            .map(|error| view! { <p class="sd-field-error">{error}</p> })
    }
}

/// A page's title, with its action, if it has one, on the right.
#[component]
fn PageHead(
    title: &'static str,
    #[prop(into)] tip: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class="sd-page-head">
            <h1 data-tip=tip>{title}</h1>
            {children.map(|children| children())}
        </header>
    }
}

/// A section of a page: its title, a note beside it, and what it holds,
/// most often a grouped list.
#[component]
fn Section(
    title: &'static str,
    #[prop(into)] tip: String,
    #[prop(optional)] note: Option<AnyView>,
    children: Children,
) -> impl IntoView {
    view! {
        <section class="sd-section">
            <div class="sd-section-head">
                <h2 data-tip=tip>{title}</h2>
                {note}
            </div>
            {children()}
        </section>
    }
}

/// A row's name, with what it does written under it, naming the field
/// `field` when it has one.
#[component]
fn Named(
    #[prop(into)] name: String,
    #[prop(into)] note: String,
    #[prop(optional, into)] field: Option<String>,
) -> impl IntoView {
    view! {
        <div class="sd-item-text">
            <label class="sd-item-name" for=field>{name}</label>
            <span class="sd-note">{note}</span>
        </div>
    }
}

/// A segmented control named `label`: a button for each of `options`, a
/// value with its title and help, the one `value` holds raised, and `set`
/// called with the one pressed.
#[component]
fn Segments<T: Copy + PartialEq + Send + Sync + 'static>(
    label: &'static str,
    options: Vec<(T, &'static str, &'static str)>,
    value: Signal<T>,
    set: Callback<T>,
    #[prop(optional, into)] id: Option<String>,
) -> impl IntoView {
    view! {
        <div class="sd-segments" role="group" aria-label=label tabindex=id.as_ref().map(|_| "-1") id=id>
            {options
                .into_iter()
                .map(|(choice, title, tip)| {
                    let chosen = move || value.get() == choice;
                    view! {
                        <button
                            data-tip=tip
                            class:sd-segment-on=chosen
                            aria-pressed=move || chosen().to_string()
                            on:click=move |_| set.run(choice)
                        >
                            {title}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// The General page: the look.
#[component]
fn General(editor: Editor) -> impl IntoView {
    let appearance = Memo::new(move |_| editor.settings.with(|s| s.general.appearance));
    let set = Callback::new(move |choice| editor.edit(|s| s.general.appearance = choice));
    view! {
        <PageHead title="General" tip=Tab::General.tip() />
        <Section title="Appearance" tip="How the app looks.">
            <div class="sd-list">
                <div class="sd-item">
                    <Named name="Appearance" note="System follows the system's look, and keeps Werewolf dark by night and light by day." />
                    <Segments
                        label="Appearance"
                        options=vec![
                            (Appearance::System, "System", "Follow the system's look. Werewolf turns dark by night and light by day."),
                            (Appearance::Light, "Light", "Always light, whatever the system or the game shows."),
                            (Appearance::Dark, "Dark", "Always dark, whatever the system or the game shows."),
                        ]
                        value=appearance.into()
                        set=set
                    />
                </div>
            </div>
        </Section>
    }
}

/// The × of a remove button, a muted stroke the button's `aria-label`
/// names.
#[component]
fn Cross() -> impl IntoView {
    view! {
        <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
            <path d="M3 3l8 8M11 3l-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
        </svg>
    }
}

/// A select among `options`, each a value and its label, over `value`,
/// which `set` changes, with the error at `at` beneath it. The select is
/// named by its `tip`.
#[component]
fn Choice(
    value: Signal<String>,
    options: Signal<Vec<(String, String)>>,
    set: Callback<String>,
    editor: Editor,
    at: Vec<String>,
    #[prop(into)] tip: String,
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
    view! {
        <span class="sd-select-box">
            <select
                class="sd-select"
                id=field_id(&at)
                aria-label=tip.clone()
                data-tip=tip
                class:sd-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:change=move |event| set.run(event_target_value(&event))
            >
                {choices}
            </select>
            <svg class="sd-chevron" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
                <path d="M3 4.5l3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
        </span>
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
        let titles: Vec<&str> = tabs().iter().map(|tab| tab.title()).collect();
        assert_eq!(titles, ["General", "Models", "Werewolf", "Avalon"]);
        let ids: Vec<String> = tabs().iter().map(|tab| tab.id()).collect();
        assert_eq!(
            ids,
            ["tab-general", "tab-models", "tab-werewolf", "tab-avalon"]
        );
        assert_eq!(
            Tab::Game(GameName::Avalon).tip(),
            "Avalon's players, phase limits, strategies and prompt parts."
        );
    }

    #[test]
    fn the_arrows_home_and_end_move_between_the_pages() {
        let avalon = Tab::Game(GameName::Avalon);
        assert_eq!(Tab::General.moved("ArrowDown"), Some(Tab::Models));
        assert_eq!(Tab::Models.moved("ArrowUp"), Some(Tab::General));
        assert_eq!(Tab::General.moved("ArrowLeft"), Some(avalon));
        assert_eq!(avalon.moved("ArrowRight"), Some(Tab::General));
        assert_eq!(Tab::Models.moved("Home"), Some(Tab::General));
        assert_eq!(Tab::Models.moved("End"), Some(avalon));
        assert_eq!(Tab::Models.moved("a"), None);
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
