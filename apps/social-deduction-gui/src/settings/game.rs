//! A game's tab: its players and phase limits side by side, its
//! strategies two across, who plays what as a grid, and its prompt parts
//! as a list beside an editor, all in one column on a narrow window.

use std::collections::BTreeMap;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use social_deduction_stage::settings::{
    GameSettings, Policy, RANDOM, RoleSettings, StrategySettings, game_key, game_title,
    is_part_name, label, part_variable,
};
use social_deduction_stage::{GameName, Role};

use super::{Choice, Cross, Editor, Problem, Tab, field_id, path};
use crate::screen::side_key;
use crate::tauri;

/// What a role does, as its help says.
pub fn role_tip(role: &str) -> &'static str {
    match role {
        "villager" => "Knows only their own role. The villagers win when every werewolf is dead.",
        "seer" => "A villager who learns one player's side each night.",
        "doctor" => "A villager who picks one player each night to save from the werewolves.",
        "werewolf" => {
            "Knows the other werewolves. Each night they kill a villager, and they win once they \
             are as many as the villagers."
        }
        "merlin" => {
            "Good. Sees every evil player but Mordred, and must stay hidden from the Assassin."
        }
        "percival" => "Good. Sees Merlin and Morgana, without knowing which is which.",
        "servant" => "Good. A loyal servant of Arthur, who knows only their own role.",
        "assassin" => {
            "Evil. Sees the other evil players but Oberon. If good wins three quests, gets one \
             guess at Merlin."
        }
        "morgana" => "Evil. Appears to Percival as Merlin might.",
        "mordred" => "Evil. Hidden from Merlin.",
        "oberon" => "Evil, but unseen by the other evil players, and sees none of them.",
        "minion" => "Evil. A minion of Mordred, who sees the other evil players but Oberon.",
        _ => "",
    }
}

/// What a phase's limit ends, as its help says.
pub fn phase_tip(phase: &str) -> &'static str {
    match phase {
        "night_limit" => {
            "How long the werewolves, the doctor and the seer have to choose. The night ends \
             sooner once all have chosen."
        }
        "day_limit" => "The longest a day can run. At the limit it ends, and nobody is voted out.",
        "ratify_limit" => {
            "How long the most-voted player must stay most-voted to be voted out. At 0s each \
             player's first vote counts instead. No longer than the day."
        }
        "propose_limit" => {
            "How long the leader has to propose a team. Proposing nobody in time fails the \
             proposal."
        }
        "vote_limit" => {
            "How long everyone has to vote on the proposed team. Not voting doesn't approve."
        }
        "quest_limit" => {
            "How long the team has to support or sabotage the quest. A member who does neither \
             supports."
        }
        "assassinate_limit" => "How long the Assassin has to name Merlin.",
        _ => "",
    }
}

/// A phase's name, from its limit's key: `day_limit` as `Day`.
pub fn phase_title(phase: &str) -> String {
    label(phase.trim_end_matches("_limit"))
}

/// How many strategies use a prompt part, `users` of them: `unused`,
/// `1 use`, `2 uses`.
pub fn uses_told(users: usize) -> String {
    match users {
        0 => "unused".to_string(),
        1 => "1 use".to_string(),
        n => format!("{n} uses"),
    }
}

/// Why `name` can't name a new prompt part when `taken` says which
/// variables the parts already have are pulled in with, if it can't.
pub fn refused_part(name: &str, taken: impl Fn(&str) -> bool) -> Option<&'static str> {
    if !is_part_name(name) {
        Some("Use letters, digits, underscores and spaces, not starting with a digit.")
    } else if taken(&part_variable(name)) {
        Some("There is already a prompt part with that name.")
    } else {
        None
    }
}

/// Why `name` can't name a new strategy when `taken` says which are
/// taken, if it can't.
pub fn refused_strategy(name: &str, taken: impl Fn(&str) -> bool) -> Option<&'static str> {
    if name == RANDOM {
        Some("random is built in.")
    } else if taken(name) {
        Some("There is already a strategy with that name.")
    } else {
        None
    }
}

/// The path of the field `rest` of `game`'s settings.
fn game_path(game: GameName, rest: &[&str]) -> Vec<String> {
    path(&[&[game_key(game)], rest].concat())
}

/// A name field and a button that adds a thing of that name with `add`,
/// unless `refuse` says why it can't, which shows beside them.
#[component]
fn Adder(
    #[prop(into)] label: String,
    #[prop(into)] placeholder: String,
    #[prop(into)] tip: String,
    #[prop(into)] button_tip: String,
    add: Callback<String>,
    refuse: Callback<String, Option<&'static str>>,
    #[prop(into)] class: String,
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
        <div class=format!("sd-adder {class}")>
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
            {move || refusal.get().map(|refusal| view! { <span class="sd-field-error">{refusal}</span> })}
        </div>
    }
}

/// A game's tab: its players, phase limits, strategies, who plays what,
/// and prompt parts, with **Restore defaults…**.
#[component]
pub(super) fn GameTab(editor: Editor, game: GameName) -> impl IntoView {
    let title = game_title(game);
    let confirming = RwSignal::new(false);
    let restore = move |_| {
        confirming.set(false);
        spawn_local(async move {
            if let Ok(defaults) = tauri::game_defaults(game).await {
                editor.edit_game(game, |settings| *settings = defaults);
            }
        });
    };
    let role_keys = editor.game(game, |s| {
        s.roles
            .iter()
            .map(|role| role.key.clone())
            .collect::<Vec<_>>()
    });
    // The roles who plays what shows: those at the table, and any other
    // given a strategy, so that its error can be shown.
    let assigned_keys = editor.game(game, |s| {
        s.roles
            .iter()
            .filter(|role| role.count > 0 || role.strategy.is_some() || !role.strategies.is_empty())
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
    let players = editor.game(game, GameSettings::players);
    let table_error = editor.error(game_path(game, &["roles"]));
    let strategy_options = Memo::new(move |_| {
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
    let model_options = Memo::new(move |_| {
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
    let refuse_strategy = Callback::new(move |name: String| {
        editor.settings.with_untracked(|s| {
            refused_strategy(&name, |name| s.game(game).strategies.contains_key(name))
        })
    });
    let everyone = editor.game(game, |s| s.strategy.clone());
    view! {
        <div class="sd-tab-head">
            <h2 data-tip=Tab::Game(game).tip()>{title}</h2>
            <Show
                when=move || confirming.get()
                fallback=move || view! {
                    <button
                        on:click=move |_| confirming.set(true)
                        data-tip=format!("Put back the {title} settings the app came with. The models stay as they are. You confirm first.")
                    >
                        "Restore defaults…"
                    </button>
                }
            >
                <div class="sd-confirm" role="alertdialog">
                    <span>{format!("Replace the {title} settings with the defaults? The models stay as they are.")}</span>
                    <button on:click=restore data-tip=format!("Replace the {title} settings with the defaults.")>"Restore"</button>
                    <button on:click=move |_| confirming.set(false) data-tip="Keep the settings as they are.">"Cancel"</button>
                </div>
            </Show>
        </div>
        <div class="sd-top">
            <section class="sd-box">
                <h3>
                    <span data-tip="How many players take each role. The total is the table's size.">"Players"</span>
                    <span class="sd-note" id=field_id(&game_path(game, &["roles"])) data-tip="Everyone at the table, every role counted.">
                        {move || format!("{} at the table", players.get())}
                    </span>
                </h3>
                <div class="sd-roles">
                    <For each=move || role_keys.get() key=Clone::clone let:role>
                        <RoleCount editor=editor game=game role=role />
                    </For>
                </div>
                <Problem error=table_error />
            </section>
            <section class="sd-box">
                <h3><span data-tip="The longest each phase can run. Write a duration, like 45s or 2m.">"Phase limits"</span></h3>
                <div class="sd-limits">
                    <For each=move || phase_keys.get() key=Clone::clone let:phase>
                        <PhaseLimit editor=editor game=game phase=phase />
                    </For>
                </div>
            </section>
        </div>
        <section class="sd-box">
            <h3>
                <span data-tip="How players decide what to do. A prompted strategy asks a model, told its system prompt. A random one chooses at random.">
                    "Strategies"
                </span>
            </h3>
            <div class="sd-strategies">
                <For each=move || strategy_names.get() key=Clone::clone let:name>
                    <StrategyCard editor=editor game=game name=name models=model_options />
                </For>
            </div>
            <Adder
                label="Add strategy"
                placeholder="Strategy name"
                tip="A name for the new strategy, like cautious or bluffer."
                button_tip="Add a prompted strategy with this name."
                add=add_strategy
                refuse=refuse_strategy
                class="sd-adder-strategy"
            />
        </section>
        <section class="sd-box">
            <h3>
                <span data-tip="Which strategy plays each role. A role left as Everyone's plays the strategy chosen for everyone.">
                    "Who plays what"
                </span>
            </h3>
            <div class="sd-assign">
                <div class="sd-everyone">
                    <Choice
                        label="Everyone"
                        label_tip="The strategy every role plays unless it is given its own."
                        tip="The strategy every role plays unless it is given its own. random chooses at random and needs no model."
                        value=everyone.into()
                        options=strategy_options.into()
                        set=Callback::new(move |name: String| editor.edit_game(game, |s| s.strategy = name))
                        editor=editor
                        at=game_path(game, &["strategy"])
                    />
                </div>
                <For each=move || assigned_keys.get() key=Clone::clone let:role>
                    <RoleStrategy editor=editor game=game role=role strategies=strategy_options.into() />
                </For>
            </div>
        </section>
        <section class="sd-box">
            <h3>
                <span data-tip="Named pieces of text that system prompts pull in with {{ name }}, like the rules, so several strategies can share them.">
                    "Prompt parts"
                </span>
            </h3>
            <PromptParts editor=editor game=game />
        </section>
    }
}

/// The role `role` of `game` in `editor`, read with `read`, as it
/// changes.
fn role_memo<T: Clone + PartialEq + Send + Sync + 'static>(
    editor: Editor,
    game: GameName,
    role: String,
    read: impl Fn(&RoleSettings) -> T + Send + Sync + 'static,
) -> Memo<Option<T>> {
    editor.game(game, move |s| {
        s.roles.iter().find(|r| r.key == role).map(&read)
    })
}

/// The style of a role's dot, in its side's color.
fn dot(role: &str) -> String {
    Role::from_key(role)
        .map(|role| format!("background: var(--sd-{})", side_key(role.side())))
        .unwrap_or_default()
}

/// A role's name, marked in its side's color, with what it does as its
/// help.
#[component]
fn RoleName(role: String) -> impl IntoView {
    view! {
        <span class="sd-role-name" data-tip=role_tip(&role)>
            <span class="sd-dot" style=dot(&role)></span>
            {label(&role)}
        </span>
    }
}

/// A role's count, as a stepper.
#[component]
fn RoleCount(editor: Editor, game: GameName, role: String) -> impl IntoView {
    let count = role_memo(editor, game, role.clone(), |r| r.count);
    let at = game_path(game, &["roles", &role, "count"]);
    let title = label(&role);
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
        <div class="sd-role">
            <RoleName role=role.clone() />
            <div class="sd-stepper">
                <button
                    aria-label=format!("Fewer {title}s")
                    data-tip=format!("One fewer {title}.")
                    on:click=move |_| down(-1)
                    disabled=move || count.get() == Some(0)
                >
                    "−"
                </button>
                <input
                    type="number"
                    min="0"
                    id=field_id(&at)
                    aria-label=format!("{title}s")
                    data-tip=format!("How many {title}s. Type a number or use − and +.")
                    prop:value=move || count.get().unwrap_or_default().to_string()
                    on:input=typed
                />
                <button aria-label=format!("More {title}s") data-tip=format!("One more {title}.") on:click=move |_| step(1)>
                    "+"
                </button>
            </div>
        </div>
        <Problem error=editor.error(at) />
    }
}

/// A phase's limit: a short field, as written.
#[component]
fn PhaseLimit(editor: Editor, game: GameName, phase: String) -> impl IntoView {
    let read = phase.clone();
    let value = editor.game(game, move |s| {
        s.phases
            .iter()
            .find(|p| p.key == read)
            .map(|p| p.limit.clone())
            .unwrap_or_default()
    });
    let written = phase.clone();
    let set = move |event| {
        let limit = event_target_value(&event);
        editor.edit_game(game, |s| {
            if let Some(phase) = s.phases.iter_mut().find(|p| p.key == written) {
                phase.limit = limit;
            }
        });
    };
    let at = game_path(game, &["phases", &phase]);
    let id = field_id(&at);
    let error = editor.error(at);
    let tip = phase_tip(&phase);
    view! {
        <div class="sd-limit">
            <label for=id.clone() data-tip=tip>{phase_title(&phase)}</label>
            <input
                type="text"
                id=id
                spellcheck="false"
                autocomplete="off"
                placeholder="45s, 2m"
                data-tip=format!("{tip} Like 45s or 2m.")
                class:sd-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=set
            />
            <Problem error=error />
        </div>
    }
}

/// Which strategy a role's seats play, and, once split, how many of them
/// play each of the others.
#[component]
fn RoleStrategy(
    editor: Editor,
    game: GameName,
    role: String,
    strategies: Signal<Vec<(String, String)>>,
) -> impl IntoView {
    let title = label(&role);
    let value = role_memo(editor, game, role.clone(), |r| r.strategy.clone());
    let value = Signal::derive(move || value.get().flatten().unwrap_or_default());
    let options = Signal::derive(move || {
        let mut options = vec![(String::new(), "Everyone's".to_string())];
        options.extend(strategies.get());
        options
    });
    let allotted = role_memo(editor, game, role.clone(), |r| r.strategies.clone());
    let split =
        RwSignal::new(allotted.with_untracked(|a| a.as_ref().is_some_and(|a| !a.is_empty())));
    let set = {
        let role = role.clone();
        Callback::new(move |name: String| {
            editor.edit_role(game, &role, |r| {
                r.strategy = (!name.is_empty()).then_some(name);
            });
        })
    };
    let at = game_path(game, &["roles", &role, "strategy"]);
    let seats = {
        let role = role.clone();
        move || {
            let allotted = allotted.get().unwrap_or_default();
            strategies
                .get()
                .into_iter()
                .map(|(name, _)| {
                    let role = role.clone();
                    let at = game_path(game, &["roles", &role, "strategies", &name]);
                    let seats = allotted.get(&name).copied().unwrap_or_default();
                    let written = name.clone();
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
                        <label class="sd-seats" data-tip=format!("How many seats of this role play {name}. 0 gives it none.")>
                            <input type="number" min="0" id=field_id(&at) prop:value=seats.to_string() on:change=set />
                            <span>{name.clone()}</span>
                        </label>
                        <Problem error=editor.error(at) />
                    }
                })
                .collect_view()
        }
    };
    view! {
        <div class="sd-assigned">
            <div class="sd-assigned-row">
                <RoleName role=role.clone() />
                <Choice
                    tip=format!("The strategy {title}s play. Everyone's is the one chosen for everyone.")
                    value=value
                    options=options
                    set=set
                    editor=editor
                    at=at
                />
                <button
                    class="sd-link"
                    aria-expanded=move || split.get().to_string()
                    data-tip="Give some of this role's seats to other strategies. The seats left play the strategy chosen here."
                    on:click=move |_| split.update(|split| *split = !*split)
                >
                    "Split seats…"
                </button>
            </div>
            <Show when=move || split.get()>
                <div class="sd-split">{seats.clone()}</div>
            </Show>
        </div>
    }
}

/// A strategy's card: a header row with its name, kind, model and
/// remove button, and, for a prompted one, its system prompt.
#[component]
fn StrategyCard(
    editor: Editor,
    game: GameName,
    name: String,
    models: Memo<Vec<(String, String)>>,
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
    let at = |rest: &[&str]| game_path(game, &[&["strategies", name.as_str()], rest].concat());
    let (system_at, model_at, policy_at, name_at) =
        (at(&["system"]), at(&["model"]), at(&["policy"]), at(&[]));
    let system_error = editor.error(system_at.clone());
    let removed = name.clone();
    let remove = move |_| {
        let removed = removed.clone();
        editor.edit_game(game, |s| {
            s.strategies.remove(&removed);
        });
    };
    let prompted = move || policy.get() == "prompted";
    view! {
        <div class="sd-card sd-strategy" id=field_id(&name_at)>
            <div class="sd-card-head">
                <h4 data-tip="The name roles are given this strategy by.">{name.clone()}</h4>
                <Choice
                    tip="Prompted: a model decides, told the system prompt. Random: every choice is made at random, and nothing is said."
                    value=policy.into()
                    options=policies
                    set=set_policy
                    editor=editor
                    at=policy_at
                />
                <Show when=prompted>
                    <Choice
                        tip="The model this strategy asks. The models are those on the Models tab."
                        value=model.into()
                        options=models.into()
                        set=set_model
                        editor=editor
                        at=model_at.clone()
                    />
                </Show>
                <button class="sd-remove" aria-label=format!("Remove {name}") data-tip="Remove this strategy. A role that plays it needs another." on:click=remove>
                    <Cross />
                </button>
            </div>
            <Problem error=editor.error(name_at.clone()) />
            <Show
                when=prompted
                fallback=|| view! { <p class="sd-note">"A random strategy needs no model or prompt."</p> }
            >
                <textarea
                    class="sd-mono"
                    id=field_id(&system_at)
                    spellcheck="false"
                    aria-label="System prompt"
                    data-tip="What the model is told before the game. It can pull in prompt parts with {{ name }}, and use {{ name }}, {{ role }} and {% if %} for the player it is playing."
                    class:sd-invalid=move || system_error.with(Option::is_some)
                    prop:value=move || system.get()
                    on:input=move |event| {
                        let text = event_target_value(&event);
                        change(&|s| s.system.clone_from(&text));
                    }
                ></textarea>
                <Problem error=system_error />
            </Show>
        </div>
    }
}

/// The prompt parts: a list of their names, each with how many
/// strategies use it, beside the editor of the one chosen.
#[component]
fn PromptParts(editor: Editor, game: GameName) -> impl IntoView {
    let names = editor.game(game, |s| s.prompt_parts.keys().cloned().collect::<Vec<_>>());
    let users = editor.game(game, |s| {
        s.part_users()
            .into_iter()
            .map(|(part, users)| {
                (
                    part.to_string(),
                    users.into_iter().map(String::from).collect(),
                )
            })
            .collect::<BTreeMap<String, Vec<String>>>()
    });
    let chosen = RwSignal::new(None::<String>);
    // The part the editor shows: the one chosen, or else the first.
    let shown = Memo::new(move |_| {
        names.with(|names| {
            chosen
                .get()
                .filter(|name| names.contains(name))
                .or_else(|| names.first().cloned())
        })
    });
    let add = Callback::new(move |name: String| {
        editor.edit_game(game, |s| {
            s.prompt_parts.entry(name.clone()).or_default();
        });
        let id = field_id(&game_path(game, &["prompt_parts", &name]));
        chosen.set(Some(name));
        // Once its editor is drawn, it is ready to type in.
        request_animation_frame(move || {
            if let Some(element) = document().get_element_by_id(&id)
                && let Ok(element) = wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(element)
            {
                let _ = element.focus();
            }
        });
    });
    let refuse = Callback::new(move |name: String| {
        editor.settings.with_untracked(|s| {
            refused_part(&name, |variable| {
                s.game(game)
                    .prompt_parts
                    .keys()
                    .any(|part| part_variable(part) == variable)
            })
        })
    });
    view! {
        <div class="sd-parts">
            <div class="sd-part-list">
                <For each=move || names.get() key=Clone::clone let:name>
                    <PartItem users=users name=name chosen=chosen shown=shown />
                </For>
                <Adder
                    label="Add prompt part"
                    placeholder="Prompt part name"
                    tip="Letters, digits, underscores and spaces, like rules or Seer only. A system prompt pulls it in with {{ name }}, spaces written as underscores."
                    button_tip="Add an empty prompt part with this name."
                    add=add
                    refuse=refuse
                    class="sd-adder-part"
                />
            </div>
            {move || match shown.get() {
                Some(name) => view! { <PartEditor editor=editor game=game name=name /> }.into_any(),
                None => view! {
                    <div class="sd-part-editor"><p class="sd-note">"No prompt parts. Add one on the left."</p></div>
                }
                .into_any(),
            }}
        </div>
    }
}

/// A prompt part in the list: its name and how many strategies use it.
#[component]
fn PartItem(
    users: Memo<BTreeMap<String, Vec<String>>>,
    name: String,
    chosen: RwSignal<Option<String>>,
    shown: Memo<Option<String>>,
) -> impl IntoView {
    let read = name.clone();
    let users =
        Memo::new(move |_| users.with(|users| users.get(&read).cloned().unwrap_or_default()));
    let (current, picked) = (name.clone(), name.clone());
    let tip = {
        let name = name.clone();
        move || {
            users.with(|users| {
                if users.is_empty() {
                    format!(
                        "No strategy pulls this in yet. Add {{{{ {} }}}} to a system prompt.",
                        part_variable(&name)
                    )
                } else {
                    format!("Pulled in by {}.", users.join(", "))
                }
            })
        }
    };
    view! {
        <button
            class="sd-part"
            aria-current=move || shown.with(|shown| shown.as_deref() == Some(current.as_str())).to_string()
            data-tip=tip
            on:click=move |_| chosen.set(Some(picked.clone()))
        >
            <span>{name}</span>
            <span class="sd-uses">{move || uses_told(users.with(Vec::len))}</span>
        </button>
    }
}

/// A prompt part's editor: the `{{ name }}` that pulls it in, its text,
/// and **Remove**.
#[component]
fn PartEditor(editor: Editor, game: GameName, name: String) -> impl IntoView {
    let read = name.clone();
    let value = editor.game(game, move |s| {
        s.prompt_parts.get(&read).cloned().unwrap_or_default()
    });
    let at = game_path(game, &["prompt_parts", &name]);
    let error = editor.error(at.clone());
    let (written, removed) = (name.clone(), name.clone());
    let pulled = format!("{{{{ {} }}}}", part_variable(&name));
    view! {
        <div class="sd-part-editor">
            <div class="sd-part-head">
                <code data-tip="Write this in a system prompt to pull in this part.">{pulled.clone()}</code>
                <span class="sd-note">"Pull it into a system prompt by writing this."</span>
                <button
                    data-tip="Remove this prompt part. A system prompt that pulls it in shows an error."
                    on:click=move |_| {
                        editor.edit_game(game, |s| {
                            s.prompt_parts.remove(&removed);
                        });
                    }
                >
                    "Remove"
                </button>
            </div>
            <textarea
                class="sd-mono"
                id=field_id(&at)
                spellcheck="false"
                aria-label=format!("Text of {name}")
                data-tip=format!("The text that replaces {pulled} wherever a system prompt uses it.")
                class:sd-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=move |event| {
                    let text = event_target_value(&event);
                    editor.edit_game(game, |s| {
                        s.prompt_parts.insert(written.clone(), text);
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
    fn every_role_and_phase_has_help() {
        for role in [
            "villager", "seer", "doctor", "werewolf", "merlin", "percival", "servant", "assassin",
            "morgana", "mordred", "oberon", "minion",
        ] {
            assert!(!role_tip(role).is_empty(), "{role}");
        }
        for phase in [
            "night_limit",
            "day_limit",
            "ratify_limit",
            "propose_limit",
            "vote_limit",
            "quest_limit",
            "assassinate_limit",
        ] {
            assert!(!phase_tip(phase).is_empty(), "{phase}");
        }
        assert_eq!(role_tip("jester"), "");
        assert_eq!(phase_tip("lunch_limit"), "");
    }

    #[test]
    fn a_phase_is_titled_without_its_limit() {
        assert_eq!(phase_title("day_limit"), "Day");
        assert_eq!(phase_title("assassinate_limit"), "Assassinate");
    }

    #[test]
    fn a_prompt_part_s_uses_are_counted() {
        assert_eq!(uses_told(0), "unused");
        assert_eq!(uses_told(1), "1 use");
        assert_eq!(uses_told(3), "3 uses");
    }

    #[test]
    fn a_new_prompt_part_needs_a_name_of_its_own_that_a_template_can_use() {
        let taken = |name: &str| name == "rules";
        assert_eq!(refused_part("tone", taken), None);
        assert_eq!(
            refused_part("rules", taken),
            Some("There is already a prompt part with that name.")
        );
        assert_eq!(refused_part("the tone", taken), None);
        assert_eq!(
            refused_part("the-tone", taken),
            Some("Use letters, digits, underscores and spaces, not starting with a digit.")
        );
        let taken = |variable: &str| variable == "Seer_only";
        assert_eq!(
            refused_part("Seer only", taken),
            Some("There is already a prompt part with that name.")
        );
    }

    #[test]
    fn a_new_strategy_needs_a_name_of_its_own() {
        let taken = |name: &str| name == "everyone";
        assert_eq!(refused_strategy("bold", taken), None);
        assert_eq!(
            refused_strategy("everyone", taken),
            Some("There is already a strategy with that name.")
        );
        assert_eq!(refused_strategy(RANDOM, taken), Some("random is built in."));
    }
}
