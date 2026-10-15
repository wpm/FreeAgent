//! The desktop app's Settings window: General, Models, Werewolf and
//! Avalon tabs, with every change stored the moment it is made and each
//! field's error in red beneath it.
//!
//! The window holds a copy of the [`AppSettings`]. Each change is made to
//! it and sent to the backend whole, which stores it and answers with what
//! is wrong. Settings changed elsewhere, by an import, come back as a
//! `settings` event, and a `focus` event, or the hash it opens at, names a
//! field to show.

use std::collections::{BTreeMap, BTreeSet};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::RichChatStyle;
use social_deduction_stage::GameName;
use social_deduction_stage::settings::{
    AppSettings, Appearance, Check, FieldError, GameSettings, KeyStatus, Listing, ModelSettings,
    Policy, Probe, Progress, Provider, RANDOM, RoleSettings, Snapshot, StrategySettings, game_key,
    game_title, is_remote, label, unique,
};
use wasm_bindgen::JsCast;

use crate::desktop::set_theme;
use crate::tauri;

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
    /// The logs folder and the look.
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

/// The provider of `providers` at `base_url`, if there is one.
pub fn provider_at<'a>(providers: &'a [Provider], base_url: &str) -> Option<&'a Provider> {
    let trimmed = base_url.trim_end_matches('/');
    providers
        .iter()
        .find(|provider| provider.base_url.trim_end_matches('/') == trimmed)
}

/// What the model id dropdown offers of `listing`: each model that
/// plays, with its mark, or, with `all`, every model, each with its mark
/// or none.
pub fn offered(listing: &Listing, all: bool) -> Vec<(String, String)> {
    listing
        .models
        .iter()
        .filter_map(|model| {
            let check = listing.checks.get(&model.id);
            let plays = check.is_some_and(Check::plays);
            if !all && !plays {
                return None;
            }
            let mark = match (check, model.hidden) {
                (Some(check), _) => check.mark(),
                (None, true) => "—",
                (None, false) => "?",
            };
            Some((model.id.clone(), mark.to_string()))
        })
        .collect()
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

    /// The error at `path`, as it changes.
    fn error(self, at: Vec<String>) -> Signal<Option<String>> {
        let errors = self.errors;
        Signal::derive(move || errors.with(|errors| error_at(errors, &at)))
    }

    /// The settings of `game`, read with `read`, as they change.
    fn game<T: Send + Sync + 'static>(
        self,
        game: GameName,
        read: impl Fn(&GameSettings) -> T + Send + Sync + 'static,
    ) -> Signal<T> {
        let settings = self.settings;
        Signal::derive(move || settings.with(|settings| read(settings.game(game))))
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
    let logs = RwSignal::new(String::new());
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
        logs.set(got.logs);
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
        <div class="sd-settings-window">
            <nav class="sd-tabs" role="tablist">
                {TABS
                    .into_iter()
                    .map(|each| {
                        view! {
                            <button
                                role="tab"
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
                    (true, Tab::General) => view! { <General editor=editor logs=logs /> }.into_any(),
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

/// The General tab: the folder logs go in, and the look.
#[component]
fn General(editor: Editor, logs: RwSignal<String>) -> impl IntoView {
    let reveal = if window()
        .navigator()
        .platform()
        .is_ok_and(|platform| platform.starts_with("Mac"))
    {
        "Show in Finder"
    } else {
        "Show in Explorer"
    };
    let change = move |_| {
        spawn_local(async move {
            if let Ok(Some(got)) = tauri::logs_choose().await {
                editor.settings.set(got.settings);
                logs.set(got.logs);
            }
        });
    };
    let appearance = move |choice: Appearance, title: &'static str| {
        let chosen = move || editor.settings.with(|s| s.general.appearance == choice);
        view! {
            <button
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
            <h2>"Game logs"</h2>
            <p class="sd-path" id=field_id(&path(&["general", "logs"]))>{move || logs.get()}</p>
            <div class="sd-row">
                <button on:click=change>"Change…"</button>
                <button on:click=|_| tauri::logs_reveal()>{reveal}</button>
            </div>
        </section>
        <section class="sd-section">
            <h2>"Appearance"</h2>
            <div class="sd-segments" role="group" aria-label="Appearance">
                {appearance(Appearance::System, "System")}
                {appearance(Appearance::Light, "Light")}
                {appearance(Appearance::Dark, "Dark")}
            </div>
            <p class="sd-note">"System follows the system's look, and keeps Werewolf dark by night and light by day."</p>
        </section>
    }
}

/// A text field over `value`, which `set` changes as it is typed, with
/// the error at `at` beneath it.
#[component]
fn TextField(
    #[prop(into)] label: String,
    value: Signal<String>,
    set: Callback<String>,
    editor: Editor,
    at: Vec<String>,
    #[prop(optional, into)] placeholder: Signal<String>,
    #[prop(optional, into)] list: Option<String>,
) -> impl IntoView {
    let error = editor.error(at.clone());
    view! {
        <label class="sd-field">
            <span>{label}</span>
            <input
                type="text"
                id=field_id(&at)
                spellcheck="false"
                autocomplete="off"
                list=list
                placeholder=move || placeholder.get()
                class:sd-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=move |event| set.run(event_target_value(&event))
            />
        </label>
        <Problem error=error />
    }
}

/// A select among `options`, each a value and its label, over `value`,
/// which `set` changes, with the error at `at` beneath it.
#[component]
fn Choice(
    #[prop(into)] label: String,
    value: Signal<String>,
    options: Signal<Vec<(String, String)>>,
    set: Callback<String>,
    editor: Editor,
    at: Vec<String>,
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
        <label class="sd-field">
            <span>{label}</span>
            <select
                class="sd-select"
                id=field_id(&at)
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

/// A name field and a button that adds a thing of that name with `add`.
#[component]
fn Adder(#[prop(into)] label: String, add: Callback<String>) -> impl IntoView {
    let field = NodeRef::<html::Input>::new();
    let added = move || {
        let Some(input) = field.get_untracked() else {
            return;
        };
        let name = input.value().trim().to_string();
        if !name.is_empty() {
            input.set_value("");
            add.run(name);
        }
    };
    view! {
        <div class="sd-row">
            <input
                type="text"
                node_ref=field
                spellcheck="false"
                placeholder="name"
                on:keydown=move |event| {
                    if event.key() == "Enter" {
                        added();
                    }
                }
            />
            <button on:click=move |_| added()>{label}</button>
        </div>
    }
}

/// A game's tab: its table, phases, strategies, who plays what, and text
/// blocks, with **Restore Defaults…**.
#[component]
fn GameTab(editor: Editor, game: GameName) -> impl IntoView {
    let key = game_key(game);
    let at = move |rest: &[&str]| path(&[&[key], rest].concat());
    let confirming = RwSignal::new(false);
    let restore = move |_| {
        confirming.set(false);
        spawn_local(async move {
            if let Ok(defaults) = tauri::game_defaults(game).await {
                editor.edit(|settings| *settings.game_mut(game) = defaults);
            }
        });
    };
    let role_keys = editor.game(game, |s| {
        s.roles
            .iter()
            .map(|role| role.key.clone())
            .collect::<Vec<_>>()
    });
    let phase_keys = editor.game(game, |s| {
        s.phases
            .iter()
            .map(|phase| phase.key.clone())
            .collect::<Vec<_>>()
    });
    let strategy_names = editor.game(game, |s| s.strategies.keys().cloned().collect::<Vec<_>>());
    let block_names = editor.game(game, |s| s.text.keys().cloned().collect::<Vec<_>>());
    let players = editor.game(game, GameSettings::players);
    let table_error = editor.error(at(&["roles"]));
    let strategy_options = Signal::derive(move || {
        let mut names: Vec<(String, String)> = strategy_names
            .get()
            .into_iter()
            .map(|name| (name.clone(), name))
            .collect();
        if !names.iter().any(|(name, _)| name == RANDOM) {
            names.push((RANDOM.to_string(), RANDOM.to_string()));
        }
        names
    });
    let model_options = Signal::derive(move || {
        let mut names: Vec<(String, String)> = vec![(String::new(), "Choose a model".to_string())];
        editor.settings.with(|s| {
            names.extend(s.models.keys().map(|name| (name.clone(), name.clone())));
        });
        names
    });
    let add_strategy = Callback::new(move |name: String| {
        editor.edit_game(game, |s| {
            s.strategies.entry(name).or_default();
        });
    });
    let add_block = Callback::new(move |name: String| {
        editor.edit_game(game, |s| {
            s.text.entry(name).or_default();
        });
    });
    let everyone = editor.game(game, |s| s.strategy.clone());
    view! {
        <div class="sd-tab-head">
            <h2>{game_title(game)}</h2>
            <Show
                when=move || confirming.get()
                fallback=move || view! { <button on:click=move |_| confirming.set(true)>"Restore Defaults…"</button> }
            >
                <div class="sd-confirm" role="alertdialog">
                    <span>{format!("Replace the {} settings with the defaults? The models stay as they are.", game_title(game))}</span>
                    <button class="sd-danger" on:click=restore>"Restore"</button>
                    <button on:click=move |_| confirming.set(false)>"Cancel"</button>
                </div>
            </Show>
        </div>
        <section class="sd-section">
            <h3>"Players"</h3>
            <For each=move || role_keys.get() key=Clone::clone let:role>
                <RoleCount editor=editor game=game role=role />
            </For>
            <p class="sd-note" id=field_id(&at(&["roles"]))>
                {move || format!("{} players at the table", players.get())}
            </p>
            <Problem error=table_error />
        </section>
        <section class="sd-section">
            <h3>"Phase limits"</h3>
            <For each=move || phase_keys.get() key=Clone::clone let:phase>
                {
                    let read = phase.clone();
                    let value = editor.game(game, move |s| {
                        s.phases.iter().find(|p| p.key == read).map(|p| p.limit.clone()).unwrap_or_default()
                    });
                    let written = phase.clone();
                    let set = Callback::new(move |limit: String| {
                        editor.edit_game(game, |s| {
                            if let Some(phase) = s.phases.iter_mut().find(|p| p.key == written) {
                                phase.limit = limit;
                            }
                        });
                    });
                    view! {
                        <TextField label=label(&phase) value=value set=set editor=editor at=at(&["phases", &phase]) placeholder="45s, 2m" />
                    }
                }
            </For>
        </section>
        <section class="sd-section">
            <h3>"Strategies"</h3>
            <For each=move || strategy_names.get() key=Clone::clone let:name>
                <StrategyCard editor=editor game=game name=name models=model_options />
            </For>
            <Adder label="Add Strategy" add=add_strategy />
        </section>
        <section class="sd-section">
            <h3>"Who plays what"</h3>
            <Choice
                label="Everyone"
                value=everyone
                options=strategy_options
                set=Callback::new(move |name: String| editor.edit_game(game, |s| s.strategy = name))
                editor=editor
                at=at(&["strategy"])
            />
            <For each=move || role_keys.get() key=Clone::clone let:role>
                <RoleStrategy editor=editor game=game role=role strategies=strategy_options />
            </For>
        </section>
        <section class="sd-section">
            <h3>"Text"</h3>
            <For each=move || block_names.get() key=Clone::clone let:block>
                <TextBlock editor=editor game=game block=block />
            </For>
            <Adder label="Add Text" add=add_block />
        </section>
    }
}

/// The role `role` of `game` in `editor`, read with `read`, as it
/// changes.
fn role_signal<T: Send + Sync + 'static>(
    editor: Editor,
    game: GameName,
    role: String,
    read: impl Fn(&RoleSettings) -> T + Send + Sync + 'static,
) -> Signal<Option<T>> {
    editor.game(game, move |s| {
        s.roles.iter().find(|r| r.key == role).map(&read)
    })
}

/// A role's count, as a stepper.
#[component]
fn RoleCount(editor: Editor, game: GameName, role: String) -> impl IntoView {
    let count = role_signal(editor, game, role.clone(), |r| r.count);
    let at = path(&[game_key(game), "roles", &role, "count"]);
    let step = {
        let role = role.clone();
        move |by: isize| {
            let role = role.clone();
            editor.edit_role(game, &role, |r| r.count = r.count.saturating_add_signed(by));
        }
    };
    let typed = {
        let role = role.clone();
        move |event| {
            let Ok(count) = event_target_value(&event).trim().parse::<usize>() else {
                return;
            };
            editor.edit_role(game, &role, |r| r.count = count);
        }
    };
    let down = step.clone();
    view! {
        <div class="sd-field">
            <span>{label(&role)}</span>
            <div class="sd-stepper">
                <button aria-label="Fewer" on:click=move |_| down(-1) disabled=move || count.get() == Some(0)>"−"</button>
                <input type="number" min="0" id=field_id(&at) prop:value=move || count.get().unwrap_or_default().to_string() on:input=typed />
                <button aria-label="More" on:click=move |_| step(1)>"+"</button>
            </div>
        </div>
        <Problem error=editor.error(at) />
    }
}

/// Which strategy a role's seats play, and how many play each of the
/// strategies it allots seats to.
#[component]
fn RoleStrategy(
    editor: Editor,
    game: GameName,
    role: String,
    strategies: Signal<Vec<(String, String)>>,
) -> impl IntoView {
    let value = role_signal(editor, game, role.clone(), |r| {
        r.strategy.clone().unwrap_or_default()
    });
    let value = Signal::derive(move || value.get().unwrap_or_default());
    let options = Signal::derive(move || {
        let mut options = vec![(String::new(), "Everyone's".to_string())];
        options.extend(strategies.get());
        options
    });
    let allotted = role_signal(editor, game, role.clone(), |r| {
        r.strategies.clone().into_iter().collect::<Vec<_>>()
    });
    let set = {
        let role = role.clone();
        Callback::new(move |name: String| {
            editor.edit_role(game, &role, |r| {
                r.strategy = (!name.is_empty()).then_some(name)
            });
        })
    };
    let at = path(&[game_key(game), "roles", &role, "strategy"]);
    let allotments = {
        let role = role.clone();
        move || {
            allotted
                .get()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, seats)| {
                    let role = role.clone();
                    let at = path(&[game_key(game), "roles", &role, "strategies", &name]);
                    let (written, shown) = (name.clone(), name.clone());
                    let set = move |event| {
                        let seats = event_target_value(&event).trim().parse::<usize>().unwrap_or_default();
                        let name = written.clone();
                        editor.edit_role(game, &role, |r| {
                            if seats == 0 {
                                r.strategies.remove(&name);
                            } else {
                                r.strategies.insert(name, seats);
                            }
                        });
                    };
                    view! {
                        <label class="sd-field sd-allotted">
                            <span>{format!("Seats playing {shown}")}</span>
                            <input type="number" min="0" id=field_id(&at) prop:value=seats.to_string() on:change=set />
                        </label>
                        <Problem error=editor.error(at) />
                    }
                })
                .collect_view()
        }
    };
    view! {
        <Choice label=label(&role) value=value options=options set=set editor=editor at=at />
        {allotments}
    }
}

/// A strategy's card: its policy, and for a prompted one, its model and
/// system prompt.
#[component]
fn StrategyCard(
    editor: Editor,
    game: GameName,
    name: String,
    models: Signal<Vec<(String, String)>>,
) -> impl IntoView {
    let read = |pick: fn(&StrategySettings) -> String| {
        let name = name.clone();
        editor.game(game, move |s| {
            s.strategies.get(&name).map(pick).unwrap_or_default()
        })
    };
    let policy = read(|s| match s.policy {
        Policy::Prompted => "prompted".to_string(),
        Policy::Random => "random".to_string(),
    });
    let model = read(|s| s.model.clone());
    let system = read(|s| s.system.clone());
    let key = StoredValue::new(name.clone());
    let change = move |change: &dyn Fn(&mut StrategySettings)| {
        editor.edit_strategy(game, &key.read_value(), change);
    };
    let policies = Signal::derive(|| {
        vec![
            ("prompted".to_string(), "Prompted".to_string()),
            ("random".to_string(), "Random".to_string()),
        ]
    });
    let set_policy = Callback::new(move |policy: String| {
        let policy = if policy == "random" {
            Policy::Random
        } else {
            Policy::Prompted
        };
        change(&|s| s.policy = policy);
    });
    let set_model = Callback::new(move |model: String| change(&|s| s.model.clone_from(&model)));
    let at = |rest: &[&str]| path(&[&[game_key(game), "strategies", name.as_str()], rest].concat());
    let (system_at, model_at, policy_at, name_at) =
        (at(&["system"]), at(&["model"]), at(&["policy"]), at(&[]));
    let system_error = editor.error(system_at.clone());
    let removed = name.clone();
    let remove = move |_| {
        let removed = removed.clone();
        editor.edit(move |settings| {
            settings.game_mut(game).strategies.remove(&removed);
        });
    };
    view! {
        <div class="sd-card" id=field_id(&name_at)>
            <div class="sd-card-head">
                <h4>{name.clone()}</h4>
                <button on:click=remove>"Remove"</button>
            </div>
            <Problem error=editor.error(name_at.clone()) />
            <Choice label="Policy" value=policy options=policies set=set_policy editor=editor at=policy_at />
            <Show when=move || policy.get() == "prompted">
                <Choice label="Model" value=model options=models set=set_model editor=editor at=model_at.clone() />
                <label class="sd-field sd-area">
                    <span>"System prompt"</span>
                    <textarea
                        id=field_id(&system_at)
                        spellcheck="false"
                        class:sd-invalid=move || system_error.with(Option::is_some)
                        prop:value=move || system.get()
                        on:input=move |event| {
                            let text = event_target_value(&event);
                            change(&|s| s.system.clone_from(&text));
                        }
                    ></textarea>
                </label>
                <Problem error=system_error />
            </Show>
        </div>
    }
}

/// A text block's card.
#[component]
fn TextBlock(editor: Editor, game: GameName, block: String) -> impl IntoView {
    let read = block.clone();
    let value = editor.game(game, move |s| {
        s.text.get(&read).cloned().unwrap_or_default()
    });
    let at = path(&[game_key(game), "text", &block]);
    let error = editor.error(at.clone());
    let (written, removed) = (block.clone(), block.clone());
    view! {
        <div class="sd-card">
            <div class="sd-card-head">
                <h4>{block.clone()}</h4>
                <button on:click=move |_| {
                    let removed = removed.clone();
                    editor.edit(move |s| {
                        s.game_mut(game).text.remove(&removed);
                    });
                }>"Remove"</button>
            </div>
            <textarea
                class="sd-mono"
                id=field_id(&at)
                spellcheck="false"
                class:sd-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=move |event| {
                    let (text, written) = (event_target_value(&event), written.clone());
                    editor.edit(move |s| {
                        s.game_mut(game).text.insert(written, text);
                    });
                }
            ></textarea>
            <Problem error=error />
        </div>
    }
}

/// A provider's listing as the window has it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Fetched {
    /// Whether it is being fetched.
    loading: bool,
    /// The listing, once it has come.
    listing: Option<Listing>,
}

/// What the Models tab knows of the providers.
#[derive(Clone, Copy)]
struct Catalog {
    /// The providers, known and local.
    providers: RwSignal<Vec<Provider>>,
    /// Each provider's listing, by base URL.
    listings: RwSignal<BTreeMap<String, Fetched>>,
    /// How far probing each provider has got, by base URL.
    progress: RwSignal<BTreeMap<String, Progress>>,
    /// Where each key variable's key is.
    keys: RwSignal<BTreeMap<String, KeyStatus>>,
}

impl Catalog {
    /// Fetch the listing of the provider at `base_url`, its key in the
    /// variable `api_key_env` names, afresh when `refresh` says.
    fn fetch(self, base_url: String, api_key_env: Option<String>, refresh: bool) {
        if base_url.trim().is_empty() {
            return;
        }
        self.listings.update(|listings| {
            let fetched = listings.entry(base_url.clone()).or_default();
            fetched.loading = true;
            if refresh {
                fetched.listing = None;
            }
        });
        spawn_local(async move {
            let listing = tauri::listing(base_url.clone(), api_key_env, refresh)
                .await
                .unwrap_or_else(|failure| Listing {
                    error: Some(failure),
                    ..Listing::default()
                });
            self.listings.update(|listings| {
                listings.insert(
                    base_url,
                    Fetched {
                        loading: false,
                        listing: Some(listing),
                    },
                );
            });
        });
    }

    /// Ask where the key `variable` names is.
    fn key_status(self, variable: String) {
        spawn_local(async move {
            let status = tauri::key_status(variable.clone())
                .await
                .unwrap_or_else(KeyStatus::Unavailable);
            self.keys.update(|keys| {
                keys.insert(variable, status);
            });
        });
    }

    /// Keep what a probe found.
    fn probed(self, probe: Probe) {
        self.listings.update(|listings| {
            let Some(listing) = listings
                .get_mut(&probe.base_url)
                .and_then(|fetched| fetched.listing.as_mut())
            else {
                return;
            };
            match probe.check {
                Some(check) => {
                    listing.checks.insert(probe.id, check);
                }
                None => {
                    listing.checks.remove(&probe.id);
                }
            }
        });
    }
}

/// The providers the Models tab shows: those known or found, and each
/// base URL a model names that is neither, as a local server when it is
/// one on this machine, whether or not it is up, and otherwise as
/// **Other**.
fn shown_providers(providers: &[Provider], settings: &AppSettings) -> Vec<Provider> {
    let mut shown = providers.to_vec();
    for model in settings.models.values() {
        if model.base_url.trim().is_empty() || provider_at(&shown, &model.base_url).is_some() {
            continue;
        }
        shown.push(match Provider::local_port(&model.base_url) {
            Some(port) => Provider::local(port),
            None => Provider {
                name: format!("Other ({})", model.base_url),
                base_url: model.base_url.clone(),
                api_key_env: model.api_key_env.clone(),
            },
        });
    }
    shown
}

/// Whether `base_url` is whole, as far as can be told while it is typed:
/// an `http` or `https` URL ending in `/v1`.
pub fn whole(base_url: &str) -> bool {
    let base_url = base_url.trim().trim_end_matches('/');
    (base_url.starts_with("http://") || base_url.starts_with("https://"))
        && base_url.ends_with("/v1")
}

/// The Models tab: the providers, with their keys, and a card for each
/// model, with **Add Model**.
#[component]
fn Models(editor: Editor) -> impl IntoView {
    let catalog = Catalog {
        providers: RwSignal::new(Vec::new()),
        listings: RwSignal::new(BTreeMap::new()),
        progress: RwSignal::new(BTreeMap::new()),
        keys: RwSignal::new(BTreeMap::new()),
    };
    spawn_local(async move {
        if let Ok(found) = tauri::providers().await {
            catalog.providers.set(found);
        }
    });
    tauri::listen("probe", move |probe: Probe| catalog.probed(probe));
    tauri::listen("progress", move |progress: Progress| {
        catalog.progress.update(|all| {
            all.insert(progress.base_url.clone(), progress);
        });
    });
    // Each provider a model uses is listed once its base URL is whole.
    let used = Memo::new(move |_| {
        editor.settings.with(|s| {
            s.models
                .values()
                .filter(|model| whole(&model.base_url))
                .map(|model| (model.base_url.clone(), model.api_key_env.clone()))
                .collect::<BTreeSet<_>>()
        })
    });
    Effect::new(move |_| {
        for (base_url, api_key_env) in used.get() {
            let fetched = catalog
                .listings
                .with_untracked(|listings| listings.contains_key(&base_url));
            if !fetched {
                catalog.fetch(base_url, api_key_env, false);
            }
        }
    });
    let shown = Memo::new(move |_| {
        catalog
            .providers
            .with(|providers| editor.settings.with(|s| shown_providers(providers, s)))
    });
    let model_names = Memo::new(move |_| {
        editor
            .settings
            .with(|s| s.models.keys().cloned().collect::<Vec<_>>())
    });
    let adding = RwSignal::new(None::<String>);
    let added_name = NodeRef::<html::Input>::new();
    let add = move |base_url: String| {
        let providers = shown.get_untracked();
        let provider = provider_at(&providers, &base_url).cloned();
        let wanted = added_name
            .get_untracked()
            .map(|input| input.value().trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "model".to_string());
        adding.set(None);
        editor.edit(move |s| {
            let name = unique(&wanted, |name| s.models.contains_key(name));
            s.models.insert(
                name,
                ModelSettings {
                    base_url: provider
                        .as_ref()
                        .map(|p| p.base_url.clone())
                        .unwrap_or_default(),
                    api_key_env: provider.and_then(|p| p.api_key_env),
                    ..ModelSettings::default()
                },
            );
        });
    };
    view! {
        <div class="sd-tab-head">
            <h2>"Models"</h2>
        </div>
        <section class="sd-section">
            <h3>"Providers"</h3>
            <For each=move || shown.get() key=|provider| provider.base_url.clone() let:provider>
                <ProviderCard catalog=catalog provider=provider />
            </For>
        </section>
        <section class="sd-section">
            <h3>"Models"</h3>
            <For each=move || model_names.get() key=Clone::clone let:name>
                <ModelCard editor=editor catalog=catalog providers=shown name=name />
            </For>
            <Show
                when=move || adding.get().is_some()
                fallback=move || view! { <div class="sd-row"><button on:click=move |_| adding.set(Some(String::new()))>"Add Model"</button></div> }
            >
                <div class="sd-card sd-adding">
                    <label class="sd-field">
                        <span>"Name"</span>
                        <input type="text" node_ref=added_name spellcheck="false" placeholder="model" />
                    </label>
                    <p class="sd-note">"Choose its provider:"</p>
                    <div class="sd-row sd-wrap">
                        {move || {
                            shown
                                .get()
                                .into_iter()
                                .map(|provider| {
                                    let base_url = provider.base_url.clone();
                                    view! { <button on:click=move |_| add(base_url.clone())>{provider.name}</button> }
                                })
                                .collect_view()
                        }}
                        <button on:click=move |_| add(String::new())>"Other…"</button>
                        <button on:click=move |_| adding.set(None)>"Cancel"</button>
                    </div>
                </div>
            </Show>
        </section>
    }
}

/// A provider's card: its base URL, its key, ↻, **Check all** for a
/// local one, how far probing has got, and why it could not list its
/// models.
#[component]
fn ProviderCard(catalog: Catalog, provider: Provider) -> impl IntoView {
    let Provider {
        name,
        base_url,
        api_key_env,
    } = provider;
    let local = !is_remote(&base_url);
    let fetched = {
        let base_url = base_url.clone();
        Memo::new(move |_| {
            catalog
                .listings
                .with(|listings| listings.get(&base_url).cloned())
        })
    };
    let progress = {
        let base_url = base_url.clone();
        move || {
            catalog.progress.with(|all| {
                all.get(&base_url)
                    .filter(|p| p.total > 0)
                    .map(Progress::told)
            })
        }
    };
    let refresh = {
        let (base_url, api_key_env) = (base_url.clone(), api_key_env.clone());
        move |_| catalog.fetch(base_url.clone(), api_key_env.clone(), true)
    };
    let check_all = {
        let (base_url, api_key_env) = (base_url.clone(), api_key_env.clone());
        move |_| tauri::check_all(base_url.clone(), api_key_env.clone())
    };
    let key = api_key_env.clone().map(|variable| {
        let (base_url, api_key_env) = (base_url.clone(), api_key_env.clone());
        let fetch = move || catalog.fetch(base_url.clone(), api_key_env.clone(), false);
        view! { <KeyField catalog=catalog variable=variable stored=Callback::new(move |()| fetch()) /> }
    });
    view! {
        <div class="sd-card sd-provider">
            <div class="sd-card-head">
                <h4>{name}</h4>
                <code class="sd-url">{base_url.clone()}</code>
                <button title="List and check its models again" aria-label="Refresh" on:click=refresh>"↻"</button>
                {local.then(|| view! { <button on:click=check_all>"Check all"</button> })}
            </div>
            {key}
            <p class="sd-note">
                {move || {
                    fetched.get().map(|fetched| match fetched.listing {
                        Some(listing) if listing.error.is_none() => format!("{} models", listing.models.len()),
                        _ if fetched.loading => "Loading models…".to_string(),
                        _ => String::new(),
                    })
                }}
                " "
                {progress}
            </p>
            {move || {
                fetched
                    .get()
                    .and_then(|fetched| fetched.listing?.error)
                    .map(|error| view! { <p class="sd-field-error">{error}</p> })
            }}
        </div>
    }
}

/// A key variable's status, and a password field to store a key, with
/// **Save**, and **Remove** when the key is in the keychain. A key is read
/// from its field when **Save** is pressed, and the field is cleared as
/// soon as the keychain answers; `stored` hears that it was stored.
#[component]
fn KeyField(catalog: Catalog, variable: String, stored: Callback<()>) -> impl IntoView {
    catalog.key_status(variable.clone());
    let status = {
        let variable = variable.clone();
        Memo::new(move |_| catalog.keys.with(|keys| keys.get(&variable).cloned()))
    };
    let field = NodeRef::<html::Input>::new();
    let failure = RwSignal::new(None::<String>);
    let save = {
        let variable = variable.clone();
        move |_| {
            let Some(input) = field.get_untracked() else {
                return;
            };
            let variable = variable.clone();
            let key = input.value();
            spawn_local(async move {
                let result = tauri::key_store(variable.clone(), key).await;
                input.set_value("");
                let ok = result.is_ok();
                failure.set(result.err());
                catalog.key_status(variable);
                if ok {
                    stored.run(());
                }
            });
        }
    };
    let remove = {
        let variable = variable.clone();
        move |_| {
            let variable = variable.clone();
            spawn_local(async move {
                failure.set(tauri::key_remove(variable.clone()).await.err());
                catalog.key_status(variable);
            });
        }
    };
    view! {
        <div class="sd-key">
            <code>{variable.clone()}</code>
            <span class="sd-note">{move || status.get().map(|status| status.told())}</span>
        </div>
        <div class="sd-row">
            <input type="password" node_ref=field autocomplete="off" placeholder="API key" />
            <button on:click=save>"Save"</button>
            <Show when=move || status.get() == Some(KeyStatus::InKeychain)>
                <button on:click=remove.clone()>"Remove"</button>
            </Show>
        </div>
        {move || failure.get().map(|failure| view! { <p class="sd-field-error">{failure}</p> })}
    }
}

/// A model's card: its provider, base URL and key variable, its id chosen
/// from the provider's listing, its context window, its probe mark and
/// **Check**.
#[component]
fn ModelCard(
    editor: Editor,
    catalog: Catalog,
    providers: Memo<Vec<Provider>>,
    name: String,
) -> impl IntoView {
    let model = {
        let name = name.clone();
        Memo::new(move |_| {
            editor
                .settings
                .with(|s| s.models.get(&name).cloned().unwrap_or_default())
        })
    };
    let key = StoredValue::new(name.clone());
    let change = move |change: &dyn Fn(&mut ModelSettings)| {
        editor.edit_model(&key.read_value(), change);
    };
    let at = |field: &str| path(&["models", name.as_str(), field]);
    let fetched = Memo::new(move |_| {
        let base_url = model.with(|m| m.base_url.clone());
        catalog
            .listings
            .with(|listings| listings.get(&base_url).cloned())
    });
    let show_all = RwSignal::new(false);
    let list_id = format!(
        "ids-{}",
        name.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    let options = move || {
        let all = show_all.get();
        fetched
            .get()
            .and_then(|fetched| fetched.listing)
            .map(|listing| offered(&listing, all))
            .unwrap_or_default()
            .into_iter()
            .map(|(id, mark)| view! { <option value=id.clone() label=format!("{mark} {id}")></option> })
            .collect_view()
    };
    let provider_value = Signal::derive(move || {
        let base_url = model.with(|m| m.base_url.clone());
        providers
            .with(|providers| provider_at(providers, &base_url).map(|p| p.base_url.clone()))
            .unwrap_or_default()
    });
    let provider_options = Signal::derive(move || {
        let mut options: Vec<(String, String)> = providers
            .get()
            .into_iter()
            .map(|p| (p.base_url, p.name))
            .collect();
        options.push((String::new(), "Other…".to_string()));
        options
    });
    let set_provider = Callback::new(move |base_url: String| {
        let provider =
            providers.with_untracked(|providers| provider_at(providers, &base_url).cloned());
        change(&|m| match &provider {
            Some(provider) => {
                m.base_url.clone_from(&provider.base_url);
                m.api_key_env.clone_from(&provider.api_key_env);
            }
            None => {
                m.base_url.clear();
                m.api_key_env = None;
            }
        });
    });
    let text = |pick: fn(&ModelSettings) -> String| Signal::derive(move || model.with(pick));
    let set_text = |write: fn(&mut ModelSettings, String)| {
        Callback::new(move |value: String| change(&|m| write(m, value.clone())))
    };
    let set_id = Callback::new(move |id: String| {
        let window = fetched.with_untracked(|fetched| {
            let listing = fetched.as_ref()?.listing.as_ref()?;
            listing.models.iter().find(|m| m.id == id)?.context_window
        });
        change(&|m| {
            m.id.clone_from(&id);
            if window.is_some() {
                m.context_window = window;
            }
        });
    });
    let window_at = at("context_window");
    let window_error = editor.error(window_at.clone());
    let set_window = move |event| {
        let window = event_target_value(&event).trim().parse::<u64>().ok();
        change(&|m| m.context_window = window);
    };
    let mark = move || {
        let id = model.with(|m| m.id.clone());
        fetched
            .get()
            .and_then(|fetched| fetched.listing)
            .and_then(|listing| listing.checks.get(&id).cloned())
            .map_or_else(
                || "unchecked".to_string(),
                |check| format!("{} {}", check.mark(), check.told()),
            )
    };
    let checking = RwSignal::new(false);
    let check = move |_| {
        let (base_url, api_key_env, id) =
            model.with_untracked(|m| (m.base_url.clone(), m.api_key_env.clone(), m.id.clone()));
        if id.is_empty() {
            return;
        }
        checking.set(true);
        spawn_local(async move {
            let _ = tauri::check_model(base_url, api_key_env, id).await;
            checking.set(false);
        });
    };
    let loading = move || {
        fetched
            .get()
            .is_some_and(|fetched| fetched.loading && fetched.listing.is_none())
    };
    let removed = name.clone();
    view! {
        <div class="sd-card">
            <div class="sd-card-head">
                <h4>{name.clone()}</h4>
                <span class="sd-mark" title="What checking the model found">{mark}</span>
                <button on:click=check disabled=move || checking.get()>
                    {move || checking.get().then(|| view! { <span class="sd-spinner"></span> })}
                    "Check"
                </button>
                <button on:click=move |_| {
                    let removed = removed.clone();
                    editor.edit(move |s| {
                        s.models.remove(&removed);
                    });
                }>"Remove"</button>
            </div>
            <Choice label="Provider" value=provider_value options=provider_options set=set_provider editor=editor at=at("provider") />
            <TextField label="Base URL" value=text(|m| m.base_url.clone()) set=set_text(|m, v| m.base_url = v) editor=editor at=at("base_url") placeholder="http://localhost:1234/v1" />
            <TextField label="Key variable" value=text(|m| m.api_key_env.clone().unwrap_or_default()) set=set_text(|m, v| m.api_key_env = (!v.trim().is_empty()).then_some(v)) editor=editor at=at("api_key_env") placeholder="none" />
            <TextField
                label="Model id"
                value=text(|m| m.id.clone())
                set=set_id
                editor=editor
                at=at("id")
                list=list_id.clone()
                placeholder=Signal::derive(move || if loading() { "Loading models…".to_string() } else { "Search or type a model id".to_string() })
            />
            <datalist id=list_id>{options}</datalist>
            {move || fetched.get().and_then(|fetched| fetched.listing?.error).map(|error| view! { <p class="sd-field-error">{error}</p> })}
            <label class="sd-check">
                <input type="checkbox" prop:checked=move || show_all.get() on:change=move |event| show_all.set(event_target_checked(&event)) />
                "Show all"
            </label>
            <label class="sd-field">
                <span>"Context window"</span>
                <input
                    type="number"
                    min="1"
                    id=field_id(&window_at)
                    placeholder="the provider's"
                    class:sd-invalid=move || window_error.with(Option::is_some)
                    prop:value=move || model.with(|m| m.context_window.map(|w| w.to_string()).unwrap_or_default())
                    on:input=set_window
                />
            </label>
            <Problem error=window_error />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use social_deduction_stage::settings::Listed;

    #[test]
    fn a_field_s_tab_is_the_first_part_of_its_path() {
        assert_eq!(Tab::of(&path(&["models", "gpt", "id"])), Tab::Models);
        assert_eq!(
            Tab::of(&path(&["werewolf", "roles"])),
            Tab::Game(GameName::Werewolf)
        );
        assert_eq!(Tab::of(&path(&["avalon"])), Tab::Game(GameName::Avalon));
        assert_eq!(Tab::of(&path(&["general", "logs"])), Tab::General);
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

    #[test]
    fn a_base_url_is_listed_once_it_is_whole() {
        assert!(whole("https://api.openai.com/v1"));
        assert!(whole("http://localhost:1234/v1/"));
        assert!(!whole("http://localhost:1234/"));
        assert!(!whole("localhost:1234/v1"));
        assert!(!whole(""));
    }

    #[test]
    fn a_provider_is_found_by_its_base_url_with_or_without_a_slash() {
        let providers = [Provider {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        }];
        assert_eq!(
            provider_at(&providers, "https://api.openai.com/v1/").map(|p| p.name.as_str()),
            Some("OpenAI")
        );
        assert!(provider_at(&providers, "http://localhost:1234/v1").is_none());
        let mut settings = AppSettings::default();
        settings.models.insert(
            "local".to_string(),
            ModelSettings {
                base_url: "http://localhost:9999/v1".to_string(),
                ..ModelSettings::default()
            },
        );
        settings.models.insert(
            "remote".to_string(),
            ModelSettings {
                base_url: "https://example.com/v1".to_string(),
                ..ModelSettings::default()
            },
        );
        settings
            .models
            .insert("blank".to_string(), ModelSettings::default());
        let shown = shown_providers(&providers, &settings);
        let names: Vec<&str> = shown.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "OpenAI",
                "Local (localhost:9999)",
                "Other (https://example.com/v1)"
            ]
        );
    }

    #[test]
    fn the_dropdown_offers_models_that_play_unless_it_shows_all() {
        let listed = |id: &str, hidden| Listed {
            id: id.to_string(),
            context_window: None,
            hidden,
        };
        let listing = Listing {
            models: vec![
                listed("plays", false),
                listed("unforced", false),
                listed("mute", false),
                listed("unchecked", false),
                listed("embedding", true),
            ],
            error: None,
            checks: BTreeMap::from([
                ("plays".to_string(), Check::Plays),
                ("unforced".to_string(), Check::PlaysUnforced),
                ("mute".to_string(), Check::DidNotPlay("no".to_string())),
            ]),
        };
        let shown = |all| -> Vec<String> {
            offered(&listing, all)
                .into_iter()
                .map(|(id, mark)| format!("{mark} {id}"))
                .collect()
        };
        assert_eq!(shown(false), ["✓ plays", "~ unforced"]);
        assert_eq!(
            shown(true),
            [
                "✓ plays",
                "~ unforced",
                "✗ mute",
                "? unchecked",
                "— embedding"
            ]
        );
    }
}
