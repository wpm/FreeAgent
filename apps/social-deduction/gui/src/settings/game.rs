//! A game's page: its players, phase limits, strategies, who plays what,
//! and prompt parts, each a section of grouped lists.

use std::collections::BTreeMap;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use social_deduction_gui_types::settings::{
    Policy, RANDOM, RoleSettings, StrategySettings, game_key, game_title, is_part_name, label,
    part_variable,
};
use social_deduction_gui_types::{GameName, Role, Side};

use free_agent_gui::chrome::Cross;

use super::{Choice, Editor, Named, PageHead, Problem, Section, Segments, Tab, field_id, path};
use crate::screen::side_key;
use crate::tauri;

/// What a role does, as written under its name. Its square tells its
/// side.
pub fn role_note(role: &str) -> &'static str {
    match role {
        "villager" => "Knows only their own role.",
        "seer" => "Learns one player's side each night.",
        "doctor" => "Picks one player each night to save from the werewolves.",
        "werewolf" => "Knows the other werewolves. Each night they kill a villager.",
        "merlin" => "Sees every evil player but Mordred, and must stay hidden from the Assassin.",
        "percival" => "Sees Merlin and Morgana, without knowing which is which.",
        "servant" => "A loyal servant of Arthur, who knows only their own role.",
        "assassin" => {
            "Sees the other evil players but Oberon. If good wins three quests, gets one guess \
             at Merlin."
        }
        "morgana" => "Appears to Percival as Merlin might.",
        "mordred" => "Hidden from Merlin.",
        "oberon" => "Unseen by the other evil players, and sees none of them.",
        "minion" => "A minion of Mordred, who sees the other evil players but Oberon.",
        _ => "",
    }
}

/// What a phase's limit ends, as written under its name.
pub fn phase_note(phase: &str) -> &'static str {
    match phase {
        "night_limit" => "How long the werewolves, the doctor and the seer have to choose.",
        "day_limit" => "The longest a day can run. At the limit nobody is voted out.",
        "ratify_limit" => "How long the most-voted player must stay most-voted to be voted out.",
        "propose_limit" => "How long the leader has to propose a team.",
        "vote_limit" => "How long everyone has to vote on the proposed team.",
        "quest_limit" => "How long the team has to support or sabotage the quest.",
        "assassinate_limit" => "How long the Assassin has to name Merlin.",
        _ => "",
    }
}

/// What more a phase's field's help tells than its note: how its limit
/// plays out, or nothing.
pub fn phase_more(phase: &str) -> &'static str {
    match phase {
        "night_limit" => "The night ends sooner once all have chosen.",
        "ratify_limit" => "At 0s each player's first vote counts instead. No longer than the day.",
        "propose_limit" => "Proposing nobody in time fails the proposal.",
        "vote_limit" => "Not voting doesn't approve.",
        "quest_limit" => "A member who does neither supports.",
        _ => "",
    }
}

/// The help of a phase's field: its note, what more there is, and how to
/// write a duration.
pub fn phase_tip(phase: &str) -> String {
    [phase_note(phase), phase_more(phase), "Like 45s or 2m."]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A phase's name, from its limit's key: `day_limit` as `Day`.
pub fn phase_title(phase: &str) -> String {
    label(phase.trim_end_matches("_limit"))
}

/// How many strategies use a prompt part, `users` of them: `unused`,
/// `used by 1 strategy`, `used by 2 strategies`.
pub fn uses_told(users: usize) -> String {
    match users {
        0 => "unused".to_string(),
        1 => "used by 1 strategy".to_string(),
        n => format!("used by {n} strategies"),
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
            {move || refusal.get().map(|refusal| view! { <span class="fa-field-error">{refusal}</span> })}
        </div>
    }
}

/// A game's page: its players, phase limits, strategies, who plays what,
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
    let players = editor.game(game, |s| s.seats().len());
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
    let table = view! {
        <span class="sd-note" id=field_id(&game_path(game, &["roles"])) data-tip="Everyone at the table, every role counted.">
            {move || format!("{} at the table", players.get())}
        </span>
    };
    let pulled = view! {
        <span class="sd-note" data-tip="Write {{ name }} in a system prompt, spaces as underscores, to pull a part in.">
            "pulled into a system prompt with {{ name }}"
        </span>
    };
    view! {
        <PageHead title=title tip=Tab::Game(game).tip()>
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
        </PageHead>
        <Section title="Players" tip="How many players take each role. The total is the table's size." note=table.into_any()>
            <div class="sd-list sd-roles">
                <For each=move || role_keys.get() key=Clone::clone let:role>
                    <RoleCount editor=editor game=game role=role />
                </For>
            </div>
            <Problem error=table_error />
        </Section>
        <Section title="Phase limits" tip="The longest each phase can run. Write a duration, like 45s or 2m.">
            <div class="sd-list sd-limits">
                <For each=move || phase_keys.get() key=Clone::clone let:phase>
                    <PhaseLimit editor=editor game=game phase=phase />
                </For>
            </div>
        </Section>
        <Section title="Strategies" tip="How players decide what to do. A prompted strategy asks a model, told its system prompt. A random one chooses at random.">
            <For each=move || strategy_names.get() key=Clone::clone let:name>
                <StrategyCard editor=editor game=game name=name models=model_options />
            </For>
            <Adder
                label="Add strategy"
                placeholder="New strategy: cautious, bluffer…"
                tip="A name for the new strategy, like cautious or bluffer."
                button_tip="Add a prompted strategy with this name."
                add=add_strategy
                refuse=refuse_strategy
                class="sd-adder-strategy"
            />
        </Section>
        <Section title="Who plays what" tip="Which strategy plays each role. A role left the same as everyone plays the strategy chosen for everyone.">
            <div class="sd-list sd-assign">
                <div class="sd-item">
                    <Named name="Everyone" note="Every role plays this unless it is given its own." />
                    <Choice
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
        </Section>
        <Section title="Prompt parts" tip="Named pieces of text that system prompts pull in with {{ name }}, like the rules, so several strategies can share them." note=pulled.into_any()>
            <PromptParts editor=editor game=game />
        </Section>
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

/// The square of a role, in its side's color, named for its side.
#[component]
fn Square(role: String) -> impl IntoView {
    let side = Role::from_key(&role).map(Role::side);
    let style = side
        .map(|side| format!("background: var(--sd-{})", side_key(side)))
        .unwrap_or_default();
    let said = side.map(|side| match side {
        Side::Good => "Good",
        Side::Evil => "Evil",
    });
    view! { <span class="sd-square" role="img" aria-label=said style=style></span> }
}

/// A role's row: its square, its name and what it does, and its count as
/// a stepper.
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
        <div class="sd-item sd-role">
            <Square role=role.clone() />
            <Named name=title.clone() note=role_note(&role) />
            <div class="fa-stepper">
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
            <Problem error=editor.error(at) />
        </div>
    }
}

/// A phase's row: its name, what its limit ends, and a short field.
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
    view! {
        <div class="sd-item sd-limit">
            <Named name=phase_title(&phase) note=phase_note(&phase) field=id.clone() />
            <input
                type="text"
                id=id
                spellcheck="false"
                autocomplete="off"
                placeholder="45s, 2m"
                data-tip=phase_tip(&phase)
                class:fa-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=set
            />
            <Problem error=error />
        </div>
    }
}

/// A role's row in who plays what: its square and name, **Split seats…**,
/// and the strategy its seats play, and, once split, how many of them
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
        let mut options = vec![(String::new(), "Same as everyone".to_string())];
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
        <div class="sd-item sd-assigned">
            <Square role=role.clone() />
            <span class="sd-item-name">{title.clone()}</span>
            <button
                class="fa-link"
                aria-expanded=move || split.get().to_string()
                data-tip="Give some of this role's seats to other strategies. The seats left play the strategy chosen here."
                on:click=move |_| split.update(|split| *split = !*split)
            >
                "Split seats…"
            </button>
            <Choice
                tip=format!("The strategy {title}s play. Same as everyone is the one chosen for everyone.")
                value=value
                options=options
                set=set
                editor=editor
                at=at
            />
            <Show when=move || split.get()>
                <div class="sd-split">{seats.clone()}</div>
            </Show>
        </div>
    }
}

/// A strategy's grouped list: its name, **Prompted | Random**, its model
/// and its remove button, and under them, for a prompted one, its system
/// prompt.
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
    let policy = {
        let name = name.clone();
        editor.game(game, move |s| {
            s.strategies
                .get(&name)
                .map(|s| s.policy)
                .unwrap_or_default()
        })
    };
    let prompted = move || policy.get() == Policy::Prompted;
    let model = read(|s| s.model.clone());
    let system = read(|s| s.system.clone());
    let key = StoredValue::new(name.clone());
    let change = move |change: &dyn Fn(&mut StrategySettings)| {
        editor.edit_strategy(game, &key.read_value(), change);
    };
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
    view! {
        <div class="sd-list sd-strategy" id=field_id(&name_at)>
            <div class="sd-item">
                <h3 class="sd-item-name" data-tip="The name roles are given this strategy by.">{name.clone()}</h3>
                <Segments
                    label="Policy"
                    options=vec![
                        (Policy::Prompted, "Prompted", "A model decides, told the system prompt."),
                        (Policy::Random, "Random", "Every choice is made at random, and nothing is said."),
                    ]
                    value=policy.into()
                    set=Callback::new(move |choice| change(&|s| s.policy = choice))
                    id=field_id(&policy_at)
                />
                <Show when=prompted>
                    <Choice
                        tip="The model this strategy asks. The models are those on the Models page."
                        value=model.into()
                        options=models.into()
                        set=set_model
                        editor=editor
                        at=model_at.clone()
                    />
                </Show>
                <button class="fa-remove" aria-label=format!("Remove {name}") data-tip="Remove this strategy. A role that plays it needs another." on:click=remove>
                    <Cross />
                </button>
                <Problem error=editor.error(policy_at.clone()) />
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
                        class:fa-invalid=move || system_error.with(Option::is_some)
                        prop:value=move || system.get()
                        on:input=move |event| {
                            let text = event_target_value(&event);
                            change(&|s| s.system.clone_from(&text));
                        }
                    ></textarea>
                    <Problem error=system_error />
                </Show>
            </div>
        </div>
    }
}

/// The prompt parts, a grouped list each, and a field to add another.
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
    let add = Callback::new(move |name: String| {
        editor.edit_game(game, |s| {
            s.prompt_parts.entry(name.clone()).or_default();
        });
        let id = field_id(&game_path(game, &["prompt_parts", &name]));
        // Once its text is drawn, it is ready to type in.
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
        <For each=move || names.get() key=Clone::clone let:name>
            <PartEditor editor=editor game=game name=name users=users />
        </For>
        <Adder
            label="Add prompt part"
            placeholder="New prompt part"
            tip="Letters, digits, underscores and spaces, like rules or Seer only. A system prompt pulls it in with {{ name }}, spaces written as underscores."
            button_tip="Add an empty prompt part with this name."
            add=add
            refuse=refuse
            class="sd-adder-part"
        />
    }
}

/// A prompt part's grouped list: the `{{ name }}` that pulls it in, how
/// many strategies use it, and **Remove**, and under them its text.
#[component]
fn PartEditor(
    editor: Editor,
    game: GameName,
    name: String,
    users: Memo<BTreeMap<String, Vec<String>>>,
) -> impl IntoView {
    let read = name.clone();
    let value = editor.game(game, move |s| {
        s.prompt_parts.get(&read).cloned().unwrap_or_default()
    });
    let read = name.clone();
    let users =
        Memo::new(move |_| users.with(|users| users.get(&read).cloned().unwrap_or_default()));
    let at = game_path(game, &["prompt_parts", &name]);
    let error = editor.error(at.clone());
    let (written, removed) = (name.clone(), name.clone());
    let pulled = format!("{{{{ {} }}}}", part_variable(&name));
    let uses_tip = {
        let pulled = pulled.clone();
        move || {
            users.with(|users| {
                if users.is_empty() {
                    format!("No strategy pulls this in yet. Add {pulled} to a system prompt.")
                } else {
                    format!("Pulled in by {}.", users.join(", "))
                }
            })
        }
    };
    view! {
        <div class="sd-list sd-part">
            <div class="sd-item">
                <code class="sd-item-name" data-tip="Write this in a system prompt to pull in this part.">{pulled.clone()}</code>
                <span class="sd-note sd-uses" data-tip=uses_tip>{move || uses_told(users.with(Vec::len))}</span>
                <button
                    class="fa-link"
                    data-tip="Remove this prompt part. A system prompt that pulls it in shows an error."
                    on:click=move |_| {
                        editor.edit_game(game, |s| {
                            s.prompt_parts.remove(&removed);
                        });
                    }
                >
                    "Remove"
                </button>
                <textarea
                    class="sd-mono"
                    id=field_id(&at)
                    spellcheck="false"
                    aria-label=format!("Text of {name}")
                    data-tip=format!("The text that replaces {pulled} wherever a system prompt uses it.")
                    class:fa-invalid=move || error.with(Option::is_some)
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
            assert!(!role_note(role).is_empty(), "{role}");
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
            assert!(!phase_note(phase).is_empty(), "{phase}");
        }
        assert_eq!(role_note("jester"), "");
        assert_eq!(phase_note("lunch_limit"), "");
    }

    #[test]
    fn a_phase_s_help_is_its_note_then_more_then_how_to_write_it() {
        assert_eq!(
            phase_tip("night_limit"),
            "How long the werewolves, the doctor and the seer have to choose. The night ends \
             sooner once all have chosen. Like 45s or 2m."
        );
        assert_eq!(
            phase_tip("assassinate_limit"),
            "How long the Assassin has to name Merlin. Like 45s or 2m."
        );
    }

    #[test]
    fn a_phase_is_titled_without_its_limit() {
        assert_eq!(phase_title("day_limit"), "Day");
        assert_eq!(phase_title("assassinate_limit"), "Assassinate");
    }

    #[test]
    fn a_prompt_part_s_uses_are_counted() {
        assert_eq!(uses_told(0), "unused");
        assert_eq!(uses_told(1), "used by 1 strategy");
        assert_eq!(uses_told(3), "used by 3 strategies");
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
