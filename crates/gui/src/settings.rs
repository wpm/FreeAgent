//! The desktop app's Settings window: a sidebar of pages, General, Models
//! and the app's own, each page a few grouped lists, with every change
//! stored the moment it is made and each field's error beneath it.
//!
//! The window holds a copy of the app's [`Settings`] in an [`Editor`].
//! Each change is made to it and sent to the backend whole, which stores
//! it and answers with what is wrong. Settings changed elsewhere, by an
//! import, come back as a `settings` event, and a `focus` event, or the
//! hash it opens at, names a field to show.
//!
//! The pieces a page is built of are here too: its [`PageHead`],
//! [`Section`]s, [`Named`] rows, [`Segments`], [`Choice`]s, [`Adder`]s and
//! the [`Problem`] under a field, and, in [`strategies`], the editors of
//! strategies and prompt parts.

use free_agent_gui_types::settings::{Appearance, FieldError, ModelSettings, Settings, Snapshot};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use wasm_bindgen::JsCast;

use crate::chrome::SelectBox;
use crate::style::FreeAgentStyle;
use crate::tauri::{call, listen};
use crate::theme::set_theme;
use crate::tip::Tips;

mod models;
pub mod strategies;

pub use models::Models;

/// Settings the window edits: an app's [`Settings`], sent to the backend
/// and back as JSON.
pub trait Edited:
    Settings + Clone + Default + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static
{
}

impl<S> Edited for S where
    S: Settings
        + Clone
        + Default
        + PartialEq
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
{
}

/// The settings, and what is wrong with them.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn settings_get<S: Edited>() -> Result<Snapshot<S>, String> {
    call("settings_get", json!({})).await
}

/// Make the settings `settings`, and what is wrong with them.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn settings_set<S: Serialize>(settings: &S) -> Result<Vec<FieldError>, String> {
    call("settings_set", json!({ "settings": settings })).await
}

/// One of the app's own pages, after General and Models.
pub trait Page: Copy + PartialEq + Send + Sync + 'static {
    /// The app's pages, in order.
    fn all() -> Vec<Self>;

    /// What the page is called.
    fn title(self) -> &'static str;

    /// What the page holds, as its help says.
    fn tip(self) -> String;

    /// What its fields' paths start with, and its tab's id ends with:
    /// `werewolf` in Social Deduction, `game` in Twenty Questions.
    fn key(self) -> &'static str;
}

/// What the General page holds, as its help says.
pub const GENERAL_TIP: &str = "The app's appearance.";

/// What the Models page holds, as its help says.
pub const MODELS_TIP: &str = "Model providers, their keys, and the models strategies can use.";

/// A page of the window, a tab of its sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab<P> {
    /// The look.
    General,
    /// The shared models and their providers.
    Models,
    /// One of the app's own pages.
    App(P),
}

impl<P: Page> Tab<P> {
    /// The pages, in order: General, Models, then the app's.
    pub fn all() -> Vec<Tab<P>> {
        [Tab::General, Tab::Models]
            .into_iter()
            .chain(P::all().into_iter().map(Tab::App))
            .collect()
    }

    /// What the tab is called.
    pub fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Models => "Models",
            Tab::App(page) => page.title(),
        }
    }

    /// What the tab holds, as its help says.
    pub fn tip(self) -> String {
        match self {
            Tab::General => GENERAL_TIP.to_string(),
            Tab::Models => MODELS_TIP.to_string(),
            Tab::App(page) => page.tip(),
        }
    }

    /// The id of the tab's button in the sidebar.
    pub fn id(self) -> String {
        match self {
            Tab::General => "tab-general".to_string(),
            Tab::Models => "tab-models".to_string(),
            Tab::App(page) => format!("tab-{}", page.key()),
        }
    }

    /// The tab a field's `path` is on.
    pub fn of(path: &[String]) -> Tab<P> {
        match path.first().map(String::as_str) {
            Some("models") => Tab::Models,
            Some(key) => P::all()
                .into_iter()
                .find(|page| page.key() == key)
                .map_or(Tab::General, Tab::App),
            None => Tab::General,
        }
    }

    /// The tab the key `key` moves to from this one, if it moves any: an
    /// arrow to the next or the previous, round the ends, and Home and End
    /// to the first and the last.
    pub fn moved(self, key: &str) -> Option<Tab<P>> {
        let tabs = Tab::all();
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

/// The field a hash names: `#settings=game/strategies/scout`, its
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
pub fn path(path: &[&str]) -> Vec<String> {
    path.iter().map(ToString::to_string).collect()
}

/// The message of the error at `path` among `errors`, if there is one.
pub fn error_at(errors: &[FieldError], path: &[String]) -> Option<String> {
    errors
        .iter()
        .find(|error| error.is_at(path))
        .map(|error| error.message.clone())
}

/// Focus the element `id`, if there is one.
pub fn focus_by_id(id: &str) {
    if let Some(element) = document()
        .get_element_by_id(id)
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = element.focus();
    }
}

/// The settings as the window holds them, and what is wrong with them.
pub struct Editor<S: 'static> {
    /// The settings.
    pub settings: RwSignal<S>,
    /// What is wrong with them, as the backend last said.
    pub errors: RwSignal<Vec<FieldError>>,
    sent: StoredValue<u64>,
}

impl<S> Clone for Editor<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for Editor<S> {}

impl<S: Edited> Editor<S> {
    /// Default settings, with nothing wrong, until they come.
    fn new() -> Self {
        Self {
            settings: RwSignal::new(S::default()),
            errors: RwSignal::new(Vec::new()),
            sent: StoredValue::new(0),
        }
    }

    /// Change the settings with `change`, and send them to be stored.
    pub fn edit(self, change: impl FnOnce(&mut S)) {
        self.settings.update(change);
        self.send();
    }

    /// Change the settings with `change`, and send them to be stored,
    /// unless it fails.
    ///
    /// # Errors
    ///
    /// What `change` failed with, the settings left as they were.
    pub fn attempt(self, change: impl FnOnce(&mut S) -> Result<(), String>) -> Result<(), String> {
        let mut changed = self.settings.get_untracked();
        change(&mut changed)?;
        self.settings.set(changed);
        self.send();
        Ok(())
    }

    /// Send the settings to be stored, and keep what is wrong with them
    /// unless a later change has been sent since.
    pub fn send(self) {
        let number = self.sent.get_value() + 1;
        self.sent.set_value(number);
        let settings = self.settings.get_untracked();
        spawn_local(async move {
            match settings_set(&settings).await {
                Ok(errors) if self.sent.get_value() == number => self.errors.set(errors),
                Ok(_) => {}
                Err(failure) => web_sys::console::error_1(&failure.into()),
            }
        });
    }

    /// Change the model `name` with `change`.
    pub fn edit_model(self, name: &str, change: impl FnOnce(&mut ModelSettings)) {
        self.edit(|settings| {
            if let Some(model) = settings.models_mut().get_mut(name) {
                change(model);
            }
        });
    }

    /// The error at `path`, telling of a change only when it changes.
    pub fn error(self, at: Vec<String>) -> Signal<Option<String>> {
        let errors = self.errors;
        Memo::new(move |_| errors.with(|errors| error_at(errors, &at))).into()
    }

    /// The settings, read with `read`, telling of a change only when what
    /// is read changes.
    pub fn read<T: Clone + PartialEq + Send + Sync + 'static>(
        self,
        read: impl Fn(&S) -> T + Send + Sync + 'static,
    ) -> Memo<T> {
        let settings = self.settings;
        Memo::new(move |_| settings.with(&read))
    }
}

/// The Settings window: General, Models, and after them, under `label`
/// when it is given, the app's own pages, each drawn by `page`. The
/// General page notes `appearance` under the appearance, and the Models
/// page lists the models that play what `plays` names.
#[component]
pub fn SettingsWindow<S: Edited, P: Page>(
    /// What the sidebar calls the app's own pages: `Games`.
    #[prop(optional)]
    label: Option<&'static str>,
    /// What is written under the appearance.
    appearance: &'static str,
    /// What a model that plays plays: `both games`.
    plays: &'static str,
    /// Draws one of the app's own pages.
    #[prop(into)]
    page: Callback<(Editor<S>, P), AnyView>,
) -> impl IntoView {
    let editor = Editor::<S>::new();
    let loaded = RwSignal::new(false);
    let opened = hashed(&window().location().hash().unwrap_or_default(), |segment| {
        js_sys::decode_uri_component(segment)
            .ok()
            .and_then(|decoded| decoded.as_string())
            .unwrap_or_else(|| segment.to_string())
    });
    let tab = RwSignal::new(opened.as_deref().map_or(Tab::General, Tab::<P>::of));
    let focus = RwSignal::new(opened);
    let take = move |got: Snapshot<S>| {
        editor.settings.set(got.settings);
        editor.errors.set(got.errors);
        loaded.set(true);
    };
    spawn_local(async move {
        match settings_get::<S>().await {
            Ok(got) => take(got),
            Err(failure) => web_sys::console::error_1(&failure.into()),
        }
    });
    listen("settings", take);
    listen("focus", move |at: Vec<String>| {
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
                }
                focus_by_id(&id);
            },
            std::time::Duration::from_millis(50),
        );
    });
    Effect::new(move |_| {
        set_theme(
            editor
                .settings
                .with(|settings| settings.general().appearance.theme()),
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
        focus_by_id(&to.id());
    };

    let button = move |each: Tab<P>| {
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
        <FreeAgentStyle />
        <Tips />
        <div class="fa-settings-window">
            <nav class="fa-sidebar" role="tablist" aria-orientation="vertical" aria-label="Settings" on:keydown=step>
                {button(Tab::General)}
                {button(Tab::Models)}
                {label.map(|label| view! { <div class="fa-sidebar-label" role="presentation">{label}</div> })}
                {P::all().into_iter().map(|each| button(Tab::App(each))).collect_view()}
            </nav>
            <main class="fa-settings fa-chrome">
                <div class="fa-page">
                    // Nothing is changed before the settings have come, so
                    // that no change overwrites them.
                    {move || match (loaded.get(), tab.get()) {
                        (false, _) => ().into_any(),
                        (true, Tab::General) => view! { <General editor=editor note=appearance /> }.into_any(),
                        (true, Tab::Models) => view! { <Models editor=editor plays=plays /> }.into_any(),
                        (true, Tab::App(each)) => page.run((editor, each)),
                    }}
                </div>
            </main>
        </div>
    }
}

/// An error beneath a field, flagged in marigold.
#[component]
pub fn Problem(
    /// The error, if there is one.
    error: Signal<Option<String>>,
) -> impl IntoView {
    move || {
        error
            .get()
            .map(|error| view! { <p class="fa-field-error">{error}</p> })
    }
}

/// A page's title, with its action, if it has one, on the right.
#[component]
pub fn PageHead(
    /// The page's title.
    title: &'static str,
    /// What the page holds.
    #[prop(into)]
    tip: String,
    /// The page's action.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class="fa-page-head">
            <h1 data-tip=tip>{title}</h1>
            {children.map(|children| children())}
        </header>
    }
}

/// A section of a page: its title, a note beside it, and what it holds,
/// most often a grouped list.
#[component]
pub fn Section(
    /// The section's title.
    title: &'static str,
    /// What the section holds.
    #[prop(into)]
    tip: String,
    /// The note beside the title.
    #[prop(optional)]
    note: Option<AnyView>,
    /// What the section holds.
    children: Children,
) -> impl IntoView {
    view! {
        <section class="fa-section">
            <div class="fa-section-head">
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
pub fn Named(
    /// The row's name.
    #[prop(into)]
    name: String,
    /// What the row does.
    #[prop(into)]
    note: String,
    /// The id of the row's field.
    #[prop(optional, into)]
    field: Option<String>,
) -> impl IntoView {
    view! {
        <div class="fa-item-text">
            <label class="fa-item-name" for=field>{name}</label>
            <span class="fa-note">{note}</span>
        </div>
    }
}

/// A segmented control named `label`: a button for each of `options`, a
/// value with its title and help, the one `value` holds raised, and `set`
/// called with the one pressed.
#[component]
pub fn Segments<T: Copy + PartialEq + Send + Sync + 'static>(
    /// What the control is called.
    label: &'static str,
    /// Each value, with its title and its help.
    options: Vec<(T, &'static str, &'static str)>,
    /// The value chosen.
    value: Signal<T>,
    /// Hears the value pressed.
    set: Callback<T>,
    /// The id of the control, which can then be focused.
    #[prop(optional, into)]
    id: Option<String>,
) -> impl IntoView {
    view! {
        <div class="fa-segments" role="group" aria-label=label tabindex=id.as_ref().map(|_| "-1") id=id>
            {options
                .into_iter()
                .map(|(choice, title, tip)| {
                    let chosen = move || value.get() == choice;
                    view! {
                        <button
                            data-tip=tip
                            class:fa-segment-on=chosen
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

/// The General page: the look, with `note` written under it.
#[component]
fn General<S: Edited>(editor: Editor<S>, note: &'static str) -> impl IntoView {
    let appearance = editor.read(|s| s.general().appearance);
    let set = Callback::new(move |choice| editor.edit(|s| s.general_mut().appearance = choice));
    view! {
        <PageHead title="General" tip=GENERAL_TIP />
        <Section title="Appearance" tip="How the app looks.">
            <div class="fa-list">
                <div class="fa-item">
                    <Named name="Appearance" note=note />
                    <Segments
                        label="Appearance"
                        options=vec![
                            (Appearance::System, "System", "Follow the system's look."),
                            (Appearance::Light, "Light", "Light, whatever the system shows."),
                            (Appearance::Dark, "Dark", "Dark, whatever the system shows."),
                        ]
                        value=appearance.into()
                        set=set
                    />
                </div>
            </div>
        </Section>
    }
}

/// A select among `options`, each a value and its label, over `value`,
/// which `set` changes, with `error` beneath it. The select is the field
/// `id`, named by its `tip`.
#[component]
pub fn Choice(
    /// The value chosen.
    value: Signal<String>,
    /// Each value and its label.
    options: Signal<Vec<(String, String)>>,
    /// Hears the value chosen.
    set: Callback<String>,
    /// The id of the select.
    #[prop(into)]
    id: String,
    /// What is wrong with the value, if anything.
    error: Signal<Option<String>>,
    /// What the select chooses.
    #[prop(into)]
    tip: String,
) -> impl IntoView {
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
        <SelectBox>
            <select
                class="fa-select"
                id=id
                aria-label=tip.clone()
                data-tip=tip
                class:fa-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:change=move |event| set.run(event_target_value(&event))
            >
                {choices}
            </select>
        </SelectBox>
        <Problem error=error />
    }
}

/// A name field and a button that adds a thing of that name with `add`,
/// unless `refuse` says why it can't, which shows beside them.
#[component]
pub fn Adder(
    /// The button's label.
    #[prop(into)]
    label: String,
    /// What the field shows while it is empty, and what it is called.
    #[prop(into)]
    placeholder: String,
    /// The field's help.
    #[prop(into)]
    tip: String,
    /// The button's help.
    #[prop(into)]
    button_tip: String,
    /// Adds a thing of the name typed.
    add: Callback<String>,
    /// Why a name can't be added, if it can't.
    refuse: Callback<String, Option<&'static str>>,
    /// The adder's own class, beside `fa-adder`.
    #[prop(into)]
    class: String,
) -> impl IntoView {
    let field = NodeRef::<html::Input>::new();
    let refusal = RwSignal::new(None::<&'static str>);
    let added = move || {
        let Some(input) = field.get_untracked() else {
            return;
        };
        let name = input.value().trim().to_string();
        if name.is_empty() {
            return;
        }
        refusal.set(refuse.run(name.clone()));
        if refusal.get_untracked().is_none() {
            input.set_value("");
            add.run(name);
        }
    };
    view! {
        <div class=format!("fa-adder {class}")>
            <input
                type="text"
                node_ref=field
                spellcheck="false"
                placeholder=placeholder.clone()
                aria-label=placeholder
                data-tip=tip
                on:input=move |_| refusal.set(None)
                on:keydown=move |event| {
                    if event.key() == "Enter" {
                        added();
                    }
                }
            />
            <button on:click=move |_| added() data-tip=button_tip>{label}</button>
            {move || refusal.get().map(|refusal| view! { <span class="fa-field-error">{refusal}</span> })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game, as an app's own page.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Game {
        Chess,
        Go,
    }

    impl Page for Game {
        fn all() -> Vec<Game> {
            vec![Game::Chess, Game::Go]
        }

        fn title(self) -> &'static str {
            match self {
                Game::Chess => "Chess",
                Game::Go => "Go",
            }
        }

        fn tip(self) -> String {
            format!("{}'s settings.", self.title())
        }

        fn key(self) -> &'static str {
            match self {
                Game::Chess => "chess",
                Game::Go => "go",
            }
        }
    }

    #[test]
    fn a_field_s_tab_is_the_first_part_of_its_path() {
        type Of = Tab<Game>;
        assert_eq!(Of::of(&path(&["models", "gpt", "id"])), Tab::Models);
        assert_eq!(Of::of(&path(&["chess", "roles"])), Tab::App(Game::Chess));
        assert_eq!(Of::of(&path(&["go"])), Tab::App(Game::Go));
        assert_eq!(Of::of(&path(&["general", "appearance"])), Tab::General);
        assert_eq!(Of::of(&[]), Tab::General);
        let titles: Vec<&str> = Of::all().iter().map(|tab| tab.title()).collect();
        assert_eq!(titles, ["General", "Models", "Chess", "Go"]);
        let ids: Vec<String> = Of::all().iter().map(|tab| tab.id()).collect();
        assert_eq!(ids, ["tab-general", "tab-models", "tab-chess", "tab-go"]);
        let tips: Vec<String> = Of::all().iter().map(|tab| tab.tip()).collect();
        assert_eq!(
            tips,
            [
                GENERAL_TIP,
                MODELS_TIP,
                "Chess's settings.",
                "Go's settings."
            ]
        );
    }

    #[test]
    fn the_arrows_home_and_end_move_between_the_pages() {
        type Of = Tab<Game>;
        let go = Tab::App(Game::Go);
        assert_eq!(Of::General.moved("ArrowDown"), Some(Tab::Models));
        assert_eq!(Of::Models.moved("ArrowUp"), Some(Tab::General));
        assert_eq!(Of::General.moved("ArrowLeft"), Some(go));
        assert_eq!(go.moved("ArrowRight"), Some(Tab::General));
        assert_eq!(Of::Models.moved("Home"), Some(Tab::General));
        assert_eq!(Of::Models.moved("End"), Some(go));
        assert_eq!(Of::Models.moved("a"), None);
    }

    #[test]
    fn the_hash_names_the_field_to_show() {
        let decode = |segment: &str| segment.replace("%20", " ");
        assert_eq!(
            hashed("#settings=chess/strategies/my%20one/model", decode),
            Some(path(&["chess", "strategies", "my one", "model"]))
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
