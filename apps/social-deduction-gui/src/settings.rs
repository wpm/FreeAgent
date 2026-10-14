//! The desktop app's settings screen, shown before a game: the
//! configuration file as a form, or as TOML, with **Open**, **Save**, the
//! presets, the API keys its models need, and **Run**.
//!
//! The backend holds the document. Each edit goes to it as a [`Value`] at
//! a TOML path, and the form redraws from the [`FormView`] it gives back.
//! A field left empty removes its key, so the game's default applies.

use std::collections::BTreeMap;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::RichChatStyle;
use social_deduction_stage::form::{Field, FormView, KeyStatus, Value, error_line, line_range};

use crate::tauri::{self, Preset};

/// The games the `game` key may name.
const GAMES: [&str; 2] = ["werewolf", "avalon"];

/// The policies a strategy may follow.
const POLICIES: [&str; 2] = ["prompted", "random"];

/// The strategy that is always defined.
const RANDOM: &str = "random";

/// An edit: the TOML path of a key, and what it is set to.
type Edit = (Vec<String>, Value);

/// `path` as a TOML path.
fn path(path: &[&str]) -> Vec<String> {
    path.iter().map(ToString::to_string).collect()
}

/// What a text field's `text` sets its key to: the text, or nothing when
/// it is empty.
fn text(text: String) -> Value {
    if text.is_empty() {
        Value::Remove
    } else {
        Value::Text(text)
    }
}

/// What a number field's `text` sets its key to: the number, or nothing
/// when it holds none.
fn number(text: String) -> Value {
    text.trim().parse().map_or(Value::Remove, Value::Integer)
}

/// Which way the configuration is shown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Form,
    Toml,
}

/// The settings screen: the configuration as a form or as TOML, and
/// **Run**, with an error under it, in full.
#[component]
pub fn Settings(
    /// The presets **Preset** lists.
    #[prop(into)]
    presets: Signal<Vec<Preset>>,
    /// Whether a game is starting: **Run** shows a spinner and is disabled.
    #[prop(into)]
    pending: Signal<bool>,
    /// Why the last game could not start.
    #[prop(into)]
    error: Signal<Option<String>>,
    /// What **Run** calls.
    run: Callback<()>,
) -> impl IntoView {
    let view = RwSignal::new(FormView::default());
    let problem = RwSignal::new(None::<String>);
    let tab = RwSignal::new(Tab::Form);
    let toml = RwSignal::new(String::new());
    let textarea = NodeRef::<html::Textarea>::new();

    // What the backend answers with: the form, or why it could not be.
    let answered = move |answer: Result<FormView, String>| match answer {
        Ok(shown) => {
            view.set(shown);
            problem.set(None);
        }
        Err(failure) => problem.set(Some(failure)),
    };
    spawn_local(async move { answered(tauri::document_view().await) });

    let edit = Callback::new(move |(path, value): Edit| {
        spawn_local(async move { answered(tauri::document_set(path, value).await) });
    });
    // The TOML tab's text, fetched afresh, with `line` selected.
    let refresh_toml = move |line: Option<usize>| {
        spawn_local(async move {
            match tauri::document_text().await {
                Ok(text) => {
                    let range = line.and_then(|line| line_range(&text, line));
                    toml.set(text);
                    if let (Some((start, end)), Some(area)) = (range, textarea.get_untracked()) {
                        let _ = area.focus();
                        let _ = area.set_selection_range(start, end);
                    }
                }
                Err(failure) => problem.set(Some(failure)),
            }
        });
    };
    let show_toml = move |line: Option<usize>| {
        tab.set(Tab::Toml);
        refresh_toml(line);
    };
    // A whole new document: the form, and the TOML tab if it shows.
    let loaded = move |answer: Result<FormView, String>| {
        answered(answer);
        if tab.get_untracked() == Tab::Toml {
            refresh_toml(None);
        }
    };
    let replace = move |event| {
        let text = event_target_value(&event);
        spawn_local(async move { answered(tauri::document_replace(text).await) });
    };

    let preset_options = move || {
        presets
            .get()
            .into_iter()
            .map(|preset| {
                let value = preset.name.clone();
                view! { <option value=value>{preset.name}</option> }
            })
            .collect_view()
    };
    let load_preset = move |event: leptos::ev::Event| {
        let name = event_target_value(&event);
        event_target::<web_sys::HtmlSelectElement>(&event).set_value("");
        if name.is_empty() {
            return;
        }
        spawn_local(async move { loaded(tauri::load_preset(name).await) });
    };
    let open = move |_| {
        spawn_local(async move {
            if let Some(answer) = tauri::open_file().await.transpose() {
                loaded(answer);
            }
        });
    };
    let save = move |_| {
        spawn_local(async move {
            if let Err(failure) = tauri::save_file().await {
                problem.set(Some(failure));
            }
        });
    };

    let error_shown = move || {
        error.get().map(|error| {
            let line = error_line(&error);
            view! {
                <pre class="sd-error">{error}</pre>
                {line.map(|line| view! {
                    <button class="sd-link" on:click=move |_| show_toml(Some(line))>
                        {format!("Show line {line} in the TOML")}
                    </button>
                })}
            }
        })
    };

    view! {
        <RichChatStyle highlight=false fonts=false />
        <div class="sd-app sd-settings-app">
            <header class="sd-header">
                <h1>"Social Deduction"</h1>
                <div class="sd-toolbar">
                    <select class="sd-select" on:change=load_preset prop:disabled=pending>
                        <option value="" selected>"Open a preset…"</option>
                        {preset_options}
                    </select>
                    <button on:click=open disabled=pending>"Open…"</button>
                    <button on:click=save>"Save…"</button>
                </div>
            </header>
            <main class="sd-settings">
                <nav class="sd-tabs">
                    <button
                        class:sd-tab-on=move || tab.get() == Tab::Form
                        on:click=move |_| tab.set(Tab::Form)
                    >
                        "Form"
                    </button>
                    <button
                        class:sd-tab-on=move || tab.get() == Tab::Toml
                        on:click=move |_| show_toml(None)
                    >
                        "TOML"
                    </button>
                </nav>
                <Show
                    when=move || tab.get() == Tab::Form
                    fallback=move || {
                        view! {
                            <textarea
                                class="sd-toml"
                                node_ref=textarea
                                spellcheck="false"
                                prop:value=move || toml.get()
                                on:change=replace
                            ></textarea>
                        }
                    }
                >
                    <Form view=view edit=edit />
                </Show>
                {move || problem.get().map(|problem| view! { <pre class="sd-error">{problem}</pre> })}
                <div class="sd-run">
                    <button
                        on:click=move |_| run.run(())
                        disabled=move || pending.get()
                    >
                        {move || pending.get().then(|| view! { <span class="sd-spinner"></span> })}
                        "Run"
                    </button>
                </div>
                {error_shown}
            </main>
        </div>
    }
}

/// The form: a section for each of the configuration's tables.
#[component]
fn Form(view: RwSignal<FormView>, edit: Callback<Edit>) -> impl IntoView {
    let game = Signal::derive(move || view.with(|view| view.game.clone()));
    let strategy = Signal::derive(move || view.with(|view| view.strategy.clone()));
    let strategy_names = Signal::derive(move || {
        let mut names: Vec<String> = view.with(|view| {
            view.strategies
                .iter()
                .map(|strategy| strategy.name.clone())
                .collect()
        });
        names.push(RANDOM.to_string());
        names
    });
    let model_names = Signal::derive(move || {
        view.with(|view| view.models.iter().map(|model| model.name.clone()).collect())
    });
    let game_options = Signal::derive(|| GAMES.map(String::from).to_vec());
    let role_keys = move || {
        view.with(|view| {
            view.roles
                .iter()
                .map(|role| role.key.clone())
                .collect::<Vec<_>>()
        })
    };

    view! {
        <div class="sd-form">
            <section>
                <h2>"Game"</h2>
                <Choice label="Game" value=game options=game_options blank="" edit=edit at=path(&["game"]) />
            </section>
            <section>
                <h2>"Players"</h2>
                <For
                    each=role_keys
                    key=Clone::clone
                    let:role
                >
                    <Role view=view role=role edit=edit />
                </For>
                <p class="sd-note">
                    {move || format!("{} players at the table", view.with(FormView::table_size))}
                </p>
            </section>
            <section>
                <h2>"Phases"</h2>
                <For
                    each=move || view.with(|view| view.phases.iter().map(|field| field.key.clone()).collect::<Vec<_>>())
                    key=Clone::clone
                    let:phase
                >
                    {
                        let value = field(view, |view| &view.phases, phase.clone());
                        let label = phase.replace('_', " ");
                        view! {
                            <TextField label=label value=value edit=edit at=path(&["phases", &phase]) placeholder="45s, 2m" />
                        }
                    }
                </For>
            </section>
            <section>
                <h2>"Strategies"</h2>
                <For
                    each=move || view.with(|view| view.strategies.iter().map(|strategy| strategy.name.clone()).collect::<Vec<_>>())
                    key=Clone::clone
                    let:name
                >
                    <StrategyCard view=view name=name models=model_names edit=edit />
                </For>
                <Adder label="Add strategy" edit=edit table="strategies" />
            </section>
            <section>
                <h2>"Who plays what"</h2>
                <Choice label="Everyone" value=strategy options=strategy_names blank="random" edit=edit at=path(&["strategy"]) />
                <For
                    each=role_keys
                    key=Clone::clone
                    let:role
                >
                    {
                        let value = Signal::derive({
                            let role = role.clone();
                            move || view.with(|view| {
                                view.roles.iter().find(|r| r.key == role).and_then(|r| r.strategy.clone())
                            })
                        });
                        view! {
                            <Choice label=role.clone() value=value options=strategy_names blank="everyone's" edit=edit at=path(&["roles", &role, "strategy"]) />
                        }
                    }
                </For>
            </section>
            <section>
                <h2>"Models"</h2>
                <For
                    each=move || view.with(|view| view.models.iter().map(|model| model.name.clone()).collect::<Vec<_>>())
                    key=Clone::clone
                    let:name
                >
                    <ModelCard view=view name=name edit=edit />
                </For>
                <Adder label="Add model" edit=edit table="models" />
            </section>
            <section>
                <h2>"Keys"</h2>
                <Keys view=view />
            </section>
            <section>
                <h2>"Text"</h2>
                <For
                    each=move || view.with(|view| view.text.iter().map(|field| field.key.clone()).collect::<Vec<_>>())
                    key=Clone::clone
                    let:block
                >
                    {
                        let value = field(view, |view| &view.text, block.clone());
                        view! {
                            <div class="sd-card">
                                <TextArea label=block.clone() value=value edit=edit at=path(&["text", &block]) />
                            </div>
                        }
                    }
                </For>
                <Adder label="Add text" edit=edit table="text" value=Value::Text("\n".to_string()) />
            </section>
        </div>
    }
}

/// The value of the field `key` among the fields `list` picks.
fn field(
    view: RwSignal<FormView>,
    list: fn(&FormView) -> &Vec<Field>,
    key: String,
) -> Signal<Option<String>> {
    Signal::derive(move || {
        view.with(|view| {
            list(view)
                .iter()
                .find(|field| field.key == key)
                .and_then(|field| field.value.clone())
        })
    })
}

/// A role's count, with the seats it comes to when the file gives none.
#[component]
fn Role(view: RwSignal<FormView>, role: String, edit: Callback<Edit>) -> impl IntoView {
    let found = {
        let role = role.clone();
        move || view.with(|view| view.roles.iter().find(|r| r.key == role).cloned())
    };
    let count = Signal::derive({
        let found = found.clone();
        move || found().and_then(|role| role.count)
    });
    let seats = move || found().map(|role| role.seats).unwrap_or_default();
    let at = path(&["roles", &role, "count"]);
    view! {
        <label class="sd-field">
            <span>{role}</span>
            <input
                type="number"
                min="0"
                prop:value=move || count.get().unwrap_or_default()
                placeholder=move || seats().to_string()
                on:change=move |event| edit.run((at.clone(), number(event_target_value(&event))))
            />
        </label>
    }
}

/// A `[strategies.*]` card: its policy, its model and its system prompt.
#[component]
fn StrategyCard(
    view: RwSignal<FormView>,
    name: String,
    models: Signal<Vec<String>>,
    edit: Callback<Edit>,
) -> impl IntoView {
    let field = |pick: fn(&social_deduction_stage::form::StrategyView) -> Option<String>| {
        let name = name.clone();
        Signal::derive(move || {
            view.with(|view| {
                view.strategies
                    .iter()
                    .find(|strategy| strategy.name == name)
                    .and_then(pick)
            })
        })
    };
    let policy = field(|strategy| strategy.policy.clone());
    let model = field(|strategy| strategy.model.clone());
    let system = field(|strategy| strategy.system.clone());
    let policies = Signal::derive(|| POLICIES.map(String::from).to_vec());
    let at = |key: &str| path(&["strategies", name.as_str(), key]);
    let (policy_at, model_at, system_at) = (at("policy"), at("model"), at("system"));
    let removed = path(&["strategies", &name]);
    view! {
        <div class="sd-card">
            <div class="sd-card-head">
                <h3>{name.clone()}</h3>
                <button on:click=move |_| edit.run((removed.clone(), Value::Remove))>"Remove"</button>
            </div>
            <Choice label="Policy" value=policy options=policies blank="" edit=edit at=policy_at />
            <Show when=move || policy.get().as_deref() == Some("prompted")>
                <Choice label="Model" value=model options=models blank="the only one" edit=edit at=model_at.clone() />
                <TextArea label="System prompt" value=system edit=edit at=system_at.clone() />
            </Show>
        </div>
    }
}

/// A `[models.*]` card: how to reach the model, and **Check**.
#[component]
fn ModelCard(view: RwSignal<FormView>, name: String, edit: Callback<Edit>) -> impl IntoView {
    let field = |pick: fn(&social_deduction_stage::form::ModelView) -> Option<String>| {
        let name = name.clone();
        Signal::derive(move || {
            view.with(|view| {
                view.models
                    .iter()
                    .find(|model| model.name == name)
                    .and_then(pick)
            })
        })
    };
    let base_url = field(|model| model.base_url.clone());
    let id = field(|model| model.id.clone());
    let api_key_env = field(|model| model.api_key_env.clone());
    let context_window = field(|model| model.context_window.clone());
    let at = |key: &str| path(&["models", name.as_str(), key]);
    let removed = path(&["models", &name]);
    let (base_url_at, id_at, key_at, window_at) = (
        at("base_url"),
        at("id"),
        at("api_key_env"),
        at("context_window"),
    );
    let checked = RwSignal::new(None::<String>);
    let checking = RwSignal::new(false);
    let check = {
        let name = name.clone();
        move |_| {
            let name = name.clone();
            checking.set(true);
            checked.set(None);
            spawn_local(async move {
                let found = tauri::model_check(name).await;
                checking.set(false);
                checked.set(Some(match found {
                    Ok(check) => check.to_string(),
                    Err(failure) => failure,
                }));
            });
        }
    };
    view! {
        <div class="sd-card">
            <div class="sd-card-head">
                <h3>{name.clone()}</h3>
                <button on:click=move |_| edit.run((removed.clone(), Value::Remove))>"Remove"</button>
            </div>
            <TextField label="Base URL" value=base_url edit=edit at=base_url_at placeholder="http://localhost:1234/v1" />
            <TextField label="Model id" value=id edit=edit at=id_at placeholder="" />
            <TextField label="Key variable" value=api_key_env edit=edit at=key_at placeholder="the provider's" />
            <label class="sd-field">
                <span>"Context window"</span>
                <input
                    type="number"
                    min="1"
                    placeholder="the provider's"
                    prop:value=move || context_window.get().unwrap_or_default()
                    on:change=move |event| edit.run((window_at.clone(), number(event_target_value(&event))))
                />
            </label>
            <div class="sd-check">
                <button on:click=check disabled=checking>
                    {move || checking.get().then(|| view! { <span class="sd-spinner"></span> })}
                    "Check"
                </button>
                <span>{move || checked.get()}</span>
            </div>
        </div>
    }
}

/// The key variables the models need, where each key is, and a password
/// field to store one. A key is read from its field when **Save** is
/// pressed, and the field is cleared as soon as the keychain answers.
#[component]
fn Keys(view: RwSignal<FormView>) -> impl IntoView {
    let statuses = RwSignal::new(BTreeMap::<String, KeyStatus>::new());
    let refresh = move |variable: String| {
        spawn_local(async move {
            let status = tauri::key_status(variable.clone())
                .await
                .unwrap_or_else(KeyStatus::Unavailable);
            statuses.update(|statuses| {
                statuses.insert(variable, status);
            });
        });
    };
    // Only a change to the variables needed asks the keychain again.
    let needed = Memo::new(move |_| view.with(|view| view.keys.clone()));
    Effect::new(move |_| {
        for need in needed.get() {
            refresh(need.variable);
        }
    });
    view! {
        <Show
            when=move || needed.with(|needed| !needed.is_empty())
            fallback=|| view! { <p class="sd-note">"No model needs a key."</p> }
        >
            <For
                each=move || needed.get()
                key=|need| (need.variable.clone(), need.models.clone())
                let:need
            >
                {
                    let variable = need.variable.clone();
                    let status = Signal::derive({
                        let variable = variable.clone();
                        move || statuses.with(|statuses| statuses.get(&variable).cloned())
                    });
                    let told = {
                        let models = need.models.clone();
                        move || status.get().map(|status| status.told(&models)).unwrap_or_default()
                    };
                    let field = NodeRef::<html::Input>::new();
                    let failure = RwSignal::new(None::<String>);
                    let store = {
                        let variable = variable.clone();
                        move |_| {
                            let Some(input) = field.get_untracked() else { return };
                            let variable = variable.clone();
                            let key = input.value();
                            spawn_local(async move {
                                let stored = tauri::key_store(variable.clone(), key).await;
                                input.set_value("");
                                failure.set(stored.err());
                                refresh(variable);
                            });
                        }
                    };
                    let remove = {
                        let variable = variable.clone();
                        move |_| {
                            let variable = variable.clone();
                            spawn_local(async move {
                                failure.set(tauri::key_remove(variable.clone()).await.err());
                                refresh(variable);
                            });
                        }
                    };
                    view! {
                        <div class="sd-card">
                            <div class="sd-card-head">
                                <h3><code>{variable}</code></h3>
                                <span class="sd-note">{told}</span>
                            </div>
                            <div class="sd-key">
                                <input type="password" node_ref=field autocomplete="off" placeholder="API key" />
                                <button on:click=store>"Save"</button>
                                <Show when=move || status.get() == Some(KeyStatus::InKeychain)>
                                    <button on:click=remove.clone()>"Remove"</button>
                                </Show>
                            </div>
                            {move || failure.get().map(|failure| view! { <p class="sd-error">{failure}</p> })}
                        </div>
                    }
                }
            </For>
        </Show>
    }
}

/// A text field over the key at `at`.
#[component]
fn TextField(
    #[prop(into)] label: String,
    value: Signal<Option<String>>,
    edit: Callback<Edit>,
    at: Vec<String>,
    #[prop(into)] placeholder: String,
) -> impl IntoView {
    view! {
        <label class="sd-field">
            <span>{label}</span>
            <input
                type="text"
                spellcheck="false"
                placeholder=placeholder
                prop:value=move || value.get().unwrap_or_default()
                on:change=move |event| edit.run((at.clone(), text(event_target_value(&event))))
            />
        </label>
    }
}

/// A monospace text area over the key at `at`.
#[component]
fn TextArea(
    #[prop(into)] label: String,
    value: Signal<Option<String>>,
    edit: Callback<Edit>,
    at: Vec<String>,
) -> impl IntoView {
    view! {
        <label class="sd-field sd-area">
            <span>{label}</span>
            <textarea
                spellcheck="false"
                prop:value=move || value.get().unwrap_or_default()
                on:change=move |event| edit.run((at.clone(), text(event_target_value(&event))))
            ></textarea>
        </label>
    }
}

/// A select over the key at `at`, among `options`, with a first option
/// that removes the key, labeled `blank`.
#[component]
fn Choice(
    #[prop(into)] label: String,
    value: Signal<Option<String>>,
    options: Signal<Vec<String>>,
    #[prop(into)] blank: String,
    edit: Callback<Edit>,
    at: Vec<String>,
) -> impl IntoView {
    let choices = move || {
        let chosen = value.get().unwrap_or_default();
        let mut options = options.get();
        // A value the file holds is shown even when it is not an option.
        if !chosen.is_empty() && !options.contains(&chosen) {
            options.push(chosen.clone());
        }
        options
            .into_iter()
            .map(|option| {
                let selected = option == chosen;
                let value = option.clone();
                view! { <option value=value selected=selected>{option}</option> }
            })
            .collect_view()
    };
    view! {
        <label class="sd-field">
            <span>{label}</span>
            <select
                class="sd-select"
                prop:value=move || value.get().unwrap_or_default()
                on:change=move |event| edit.run((at.clone(), text(event_target_value(&event))))
            >
                <option value="">{blank}</option>
                {choices}
            </select>
        </label>
    }
}

/// A name field and a button that sets the key of that name under
/// `table` to `value`: an empty table unless it says otherwise.
#[component]
fn Adder(
    #[prop(into)] label: String,
    edit: Callback<Edit>,
    table: &'static str,
    #[prop(default = Value::Table)] value: Value,
) -> impl IntoView {
    let field = NodeRef::<html::Input>::new();
    let add = move |_| {
        let Some(input) = field.get_untracked() else {
            return;
        };
        let name = input.value().trim().to_string();
        if name.is_empty() {
            return;
        }
        input.set_value("");
        edit.run((path(&[table, &name]), value.clone()));
    };
    view! {
        <div class="sd-adder">
            <input type="text" node_ref=field spellcheck="false" placeholder="name" />
            <button on:click=add>{label}</button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_left_empty_removes_its_key_and_a_number_field_sets_a_number() {
        assert_eq!(text("45s".to_string()), Value::Text("45s".to_string()));
        assert_eq!(text(String::new()), Value::Remove);
        assert_eq!(number(" 3 ".to_string()), Value::Integer(3));
        assert_eq!(number(String::new()), Value::Remove);
        assert_eq!(number("many".to_string()), Value::Remove);
    }

    #[test]
    fn a_path_is_its_keys_in_order() {
        assert_eq!(
            path(&["roles", "seer", "count"]),
            ["roles", "seer", "count"]
        );
    }
}
