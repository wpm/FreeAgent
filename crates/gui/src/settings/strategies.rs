//! The editors of an app's [`Strategies`]: a card for each strategy, its
//! model and its system prompt, and the prompt parts the system prompts
//! pull in, with a field to add another.

use std::collections::BTreeMap;

use free_agent_gui_types::settings::{
    Policy, RANDOM, Strategies, StrategySettings, is_part_name, part_variable,
};
use leptos::prelude::*;

use super::{Adder, Choice, Edited, Editor, Problem, Segments, field_id, focus_by_id};
use crate::chrome::Cross;

/// How many use a prompt part, `users` of them, each `called` as one and
/// as many: `unused`, `used by 1 strategy`, `used by 2 strategies`.
pub(crate) fn uses_told(users: usize, (one, many): (&str, &str)) -> String {
    match users {
        0 => "unused".to_string(),
        1 => format!("used by 1 {one}"),
        n => format!("used by {n} {many}"),
    }
}

/// Why `name` can't name a new prompt part when `taken` says which
/// variables the parts already have are pulled in with, if it can't.
pub(crate) fn refused_part(name: &str, taken: impl Fn(&str) -> bool) -> Option<&'static str> {
    if !is_part_name(name) {
        Some("Use letters, digits, underscores and spaces, not starting with a digit.")
    } else if taken(&part_variable(name)) {
        Some("There is already a prompt part with that name.")
    } else {
        None
    }
}

/// Why `name` can't name a new strategy when `taken` says which are
/// taken, if it can't. Where a strategy `may_be_random`, [`RANDOM`] is
/// built in.
pub(crate) fn refused_strategy(
    name: &str,
    may_be_random: bool,
    taken: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    if may_be_random && name == RANDOM {
        Some("random is built in.")
    } else if taken(name) {
        Some("There is already a strategy with that name.")
    } else {
        None
    }
}

/// The path of the field `rest` of the strategies and prompt parts of
/// `of`.
pub fn path_of<S: Strategies>(of: S::Of, rest: &[&str]) -> Vec<String> {
    let mut path = S::at(of);
    path.extend(rest.iter().map(ToString::to_string));
    path
}

/// A strategy's grouped list: its name, **Prompted | Random** where a
/// strategy may be random, its model and its remove button, and under
/// them, for a prompted one, its system prompt. `models` are the models
/// to choose from, and the tips say what the name, the remove button and
/// the system prompt are in the app's words.
#[component]
pub(crate) fn StrategyCard<S: Strategies + Edited>(
    /// The settings.
    editor: Editor<S>,
    /// Whose strategy it is.
    of: S::Of,
    /// The strategy's name.
    name: String,
    /// Each model to choose from, and its label.
    models: Memo<Vec<(String, String)>>,
    /// What the strategy's name is for.
    name_tip: &'static str,
    /// What removing the strategy does.
    remove_tip: &'static str,
    /// What the system prompt is, and what it can pull in.
    system_tip: &'static str,
) -> impl IntoView {
    let strategy = {
        let name = name.clone();
        move |pick: fn(&StrategySettings) -> String| {
            let name = name.clone();
            editor.read(move |s| s.strategies(of).get(&name).map(pick).unwrap_or_default())
        }
    };
    let policy = {
        let name = name.clone();
        editor.read(move |s| {
            s.strategies(of)
                .get(&name)
                .map(|s| s.policy)
                .unwrap_or_default()
        })
    };
    let prompted = move || policy.get() == Policy::Prompted;
    let model = strategy(|s| s.model.clone());
    let system = strategy(|s| s.system.clone());
    let key = StoredValue::new(name.clone());
    let change = move |change: &dyn Fn(&mut StrategySettings)| {
        editor.edit(|s| {
            if let Some(strategy) = s.strategies_mut(of).get_mut(&*key.read_value()) {
                change(strategy);
            }
        });
    };
    let set_model = Callback::new(move |model: String| change(&|s| s.model.clone_from(&model)));
    let path = |rest: &[&str]| path_of::<S>(of, &[&["strategies", name.as_str()], rest].concat());
    let (system_at, model_at, policy_at, name_at) = (
        path(&["system"]),
        path(&["model"]),
        path(&["policy"]),
        path(&[]),
    );
    let system_error = editor.error(system_at.clone());
    let removed = name.clone();
    let remove = move |_| {
        let removed = removed.clone();
        editor.edit(|s| {
            s.strategies_mut(of).remove(&removed);
        });
    };
    let policies = S::MAY_BE_RANDOM.then(|| {
        view! {
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
        }
    });
    view! {
        <div class="fa-list fa-strategy" id=field_id(&name_at)>
            <div class="fa-item">
                <h3 class="fa-item-name" data-tip=name_tip>{name.clone()}</h3>
                {policies}
                <Show when=prompted>
                    <Choice
                        tip="The model this strategy asks. The models are those on the Models page."
                        value=model.into()
                        options=models.into()
                        set=set_model
                        id=field_id(&model_at)
                        error=editor.error(model_at.clone())
                    />
                </Show>
                <button class="fa-remove" aria-label=format!("Remove {name}") data-tip=remove_tip on:click=remove>
                    <Cross />
                </button>
                <Problem error=editor.error(policy_at.clone()) />
                <Problem error=editor.error(name_at.clone()) />
                <Show
                    when=prompted
                    fallback=|| view! { <p class="fa-note">"A random strategy needs no model or prompt."</p> }
                >
                    <textarea
                        class="fa-mono"
                        id=field_id(&system_at)
                        spellcheck="false"
                        aria-label="System prompt"
                        data-tip=system_tip
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

/// The strategies of `of`, a [`StrategyCard`] each, and a field to add
/// another, a prompted strategy. `models` and the tips are the cards'.
/// The cards sit in the section as it lays them out, or, where `class`
/// says, as it does.
#[component]
pub fn StrategyList<S: Strategies + Edited>(
    /// The settings.
    editor: Editor<S>,
    /// Whose strategies they are.
    of: S::Of,
    /// Each model to choose from, and its label.
    models: Memo<Vec<(String, String)>>,
    /// What a strategy's name is for.
    name_tip: &'static str,
    /// What removing a strategy does.
    remove_tip: &'static str,
    /// What a system prompt is, and what it can pull in.
    system_tip: &'static str,
    /// Classes for the cards' box besides `fa-strategies`.
    #[prop(optional)]
    class: &'static str,
) -> impl IntoView {
    let names = editor.read(move |s| s.strategies(of).keys().cloned().collect::<Vec<_>>());
    let add = Callback::new(move |name: String| {
        editor.edit(|s| {
            s.strategies_mut(of).entry(name).or_default();
        });
    });
    let refuse = Callback::new(move |name: String| {
        editor.settings.with_untracked(|s| {
            refused_strategy(&name, S::MAY_BE_RANDOM, |name| {
                s.strategies(of).contains_key(name)
            })
        })
    });
    view! {
        <div class=format!("fa-strategies {class}")>
            <For each=move || names.get() key=Clone::clone let:name>
                <StrategyCard
                    editor=editor
                    of=of
                    name=name
                    models=models
                    name_tip=name_tip
                    remove_tip=remove_tip
                    system_tip=system_tip
                />
            </For>
        </div>
        <Adder
            label="Add strategy"
            placeholder="New strategy: cautious, bluffer…"
            tip="A name for the new strategy, like cautious or bluffer."
            button_tip="Add a prompted strategy with this name."
            add=add
            refuse=refuse
            class="fa-adder-strategy"
        />
    }
}

/// The prompt parts of `of`, a grouped list each, and a field to add
/// another.
#[component]
pub fn PromptParts<S: Strategies + Edited>(
    /// The settings.
    editor: Editor<S>,
    /// Whose prompt parts they are.
    of: S::Of,
) -> impl IntoView {
    let names = editor.read(move |s| s.prompt_parts(of).keys().cloned().collect::<Vec<_>>());
    let users = editor.read(move |s| s.part_users(of));
    let add = Callback::new(move |name: String| {
        editor.edit(|s| {
            s.prompt_parts_mut(of).entry(name.clone()).or_default();
        });
        let id = field_id(&path_of::<S>(of, &["prompt_parts", &name]));
        // Once its text is drawn, it is ready to type in.
        request_animation_frame(move || focus_by_id(&id));
    });
    let refuse = Callback::new(move |name: String| {
        editor.settings.with_untracked(|s| {
            refused_part(&name, |variable| {
                s.prompt_parts(of)
                    .keys()
                    .any(|part| part_variable(part) == variable)
            })
        })
    });
    view! {
        <For each=move || names.get() key=Clone::clone let:name>
            <PartEditor editor=editor of=of name=name users=users />
        </For>
        <Adder
            label="Add prompt part"
            placeholder="New prompt part"
            tip="Letters, digits, underscores and spaces, like rules or Seer only. A system prompt pulls it in with {{ name }}, spaces written as underscores."
            button_tip="Add an empty prompt part with this name."
            add=add
            refuse=refuse
            class="fa-adder-part"
        />
    }
}

/// A prompt part's grouped list: the `{{ name }}` that pulls it in, how
/// many strategies use it, and **Remove**, and under them its text.
#[component]
fn PartEditor<S: Strategies + Edited>(
    editor: Editor<S>,
    of: S::Of,
    name: String,
    users: Memo<BTreeMap<String, Vec<String>>>,
) -> impl IntoView {
    let value = {
        let name = name.clone();
        editor.read(move |s| s.prompt_parts(of).get(&name).cloned().unwrap_or_default())
    };
    let read = name.clone();
    let users =
        Memo::new(move |_| users.with(|users| users.get(&read).cloned().unwrap_or_default()));
    let at = path_of::<S>(of, &["prompt_parts", &name]);
    let error = editor.error(at.clone());
    let (written, removed) = (name.clone(), name.clone());
    let pulled = format!("{{{{ {} }}}}", part_variable(&name));
    let uses_tip = {
        let pulled = pulled.clone();
        let one = S::PART_USERS.0;
        move || {
            users.with(|users| {
                if users.is_empty() {
                    format!("No {one} pulls this in yet. Add {pulled} to a system prompt.")
                } else {
                    format!("Pulled in by {}.", users.join(", "))
                }
            })
        }
    };
    view! {
        <div class="fa-list fa-part">
            <div class="fa-item">
                <code class="fa-item-name" data-tip="Write this in a system prompt to pull in this part.">{pulled.clone()}</code>
                <span class="fa-note fa-uses" data-tip=uses_tip>{move || uses_told(users.with(Vec::len), S::PART_USERS)}</span>
                <button
                    class="fa-link"
                    data-tip="Remove this prompt part. A system prompt that pulls it in shows an error."
                    on:click=move |_| {
                        editor.edit(|s| {
                            s.prompt_parts_mut(of).remove(&removed);
                        });
                    }
                >
                    "Remove"
                </button>
                <textarea
                    class="fa-mono"
                    id=field_id(&at)
                    spellcheck="false"
                    aria-label=format!("Text of {name}")
                    data-tip=format!("The text that replaces {pulled} wherever a system prompt uses it.")
                    class:fa-invalid=move || error.with(Option::is_some)
                    prop:value=move || value.get()
                    on:input=move |event| {
                        let text = event_target_value(&event);
                        editor.edit(|s| {
                            s.prompt_parts_mut(of).insert(written.clone(), text);
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
    fn a_prompt_part_s_uses_are_counted() {
        let called = ("strategy", "strategies");
        assert_eq!(uses_told(0, called), "unused");
        assert_eq!(uses_told(1, called), "used by 1 strategy");
        assert_eq!(uses_told(3, called), "used by 3 strategies");
        assert_eq!(uses_told(2, ("prompt", "prompts")), "used by 2 prompts");
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
        assert_eq!(refused_strategy("bold", true, taken), None);
        assert_eq!(
            refused_strategy("everyone", true, taken),
            Some("There is already a strategy with that name.")
        );
        assert_eq!(
            refused_strategy(RANDOM, true, taken),
            Some("random is built in.")
        );
        assert_eq!(refused_strategy(RANDOM, false, taken), None);
    }
}
