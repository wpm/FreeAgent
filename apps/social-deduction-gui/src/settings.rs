//! The desktop app's Settings window: General, Models, Werewolf and
//! Avalon tabs, with every change stored the moment it is made and each
//! field's error in red beneath it.
//!
//! The window holds a copy of the [`AppSettings`]. Each change is made to
//! it and sent to the backend whole, which stores it and answers with what
//! is wrong. Settings changed elsewhere, by an import, come back as a
//! `settings` event, and a `focus` event, or the hash it opens at, names a
//! field to show.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::RichChatStyle;
use social_deduction_stage::GameName;
use social_deduction_stage::settings::{
    AppSettings, Appearance, FieldError, GameSettings, ModelSettings, Policy, RANDOM, RoleSettings,
    Snapshot, StrategySettings, game_key, game_title, label,
};
use wasm_bindgen::JsCast;

use crate::desktop::set_theme;
use crate::tauri;

mod models;

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
fn Adder(
    #[prop(into)] label: String,
    add: Callback<String>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
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
        <div class=format!("sd-row {class}")>
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
            <Adder label="Add Text" add=add_block class="sd-adder-part" />
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
}
