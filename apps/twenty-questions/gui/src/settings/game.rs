//! The Game page: the whole configuration a game is played from, but its
//! models. The players, their guesses and the time limit; the host, its
//! secret object and how it picks one; the strategies, who plays what,
//! and the prompt parts, each a section of grouped lists.

use free_agent_gui::settings::strategies::{PromptParts, StrategyList, path_of};
use free_agent_gui::settings::{
    Choice, Editor, Named, Page, PageHead, Problem, RestoreDefaults, Section, field_id,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use twenty_questions_gui_types::settings::{AppSettings, GUESSES, GameSettings, NAMES, label};

use super::GamePage;
use crate::tauri;

/// The path of the field `rest` of the game's settings.
fn game_path(rest: &[&str]) -> Vec<String> {
    path_of::<AppSettings>((), rest)
}

/// `names` as a list in prose: `Alice, Bob and Carol`.
fn in_prose(names: &[&str]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

/// The categories as the field shows them, one per line.
fn categories_text(categories: &[String]) -> String {
    categories.join("\n")
}

/// The categories the field's `text` holds, one per line, kept as typed so
/// that the field reads back what was typed.
fn categories_of(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').map(ToString::to_string).collect()
    }
}

/// `count` moved `by`, kept from 1 to `most`.
fn stepped(count: usize, by: isize, most: usize) -> usize {
    count.saturating_add_signed(by).clamp(1, most)
}

/// The game's settings, read with `read`, as they change.
fn game<T: Clone + PartialEq + Send + Sync + 'static>(
    editor: Editor<AppSettings>,
    read: impl Fn(&GameSettings) -> T + Send + Sync + 'static,
) -> Memo<T> {
    editor.read(move |settings| read(&settings.game))
}

/// Change the game's settings with `change`.
fn edit_game(editor: Editor<AppSettings>, change: impl FnOnce(&mut GameSettings)) {
    editor.edit(|settings| change(&mut settings.game));
}

/// The Game page: the players, the host, the strategies, who plays what
/// and the prompt parts, with **Restore defaults…**.
#[component]
pub(super) fn GameTab(editor: Editor<AppSettings>) -> impl IntoView {
    let restore = move || {
        spawn_local(async move {
            if let Ok(defaults) = tauri::game_defaults().await {
                edit_game(editor, |settings| *settings = defaults);
            }
        });
    };
    let model_options = Memo::new(move |_| {
        let mut names: Vec<(String, String)> = vec![(String::new(), "Choose a model".to_string())];
        editor.settings.with(|s| {
            names.extend(s.models.keys().map(|name| (name.clone(), name.clone())));
        });
        names
    });
    let pulled = view! {
        <span class="fa-note" data-tip="Write {{ name }} in a prompt, spaces as underscores, to pull a part in.">
            "pulled into a prompt with {{ name }}"
        </span>
    };
    view! {
        <PageHead title="Game" tip=GamePage.tip()>
            <RestoreDefaults what="the game settings" restore=restore />
        </PageHead>
        <span id=field_id(&game_path(&[])) tabindex="-1"></span>
        <Problem error=editor.error(game_path(&[])) />
        <Section title="Players" tip="The model players at the table, Alice to Leo, each in a color of their own, and what they have to find the object.">
            <div class="fa-list">
                <Players editor=editor />
                <Guesses editor=editor />
                <Text
                    editor=editor
                    at=&["time_limit"]
                    name="Time limit"
                    note="When it runs out, the host wins."
                    placeholder="10m"
                    tip="How long the game lasts, like 10m or 90s. At least 30s."
                    read=|s| s.time_limit.clone()
                    write=|s, text| s.time_limit = text
                />
            </div>
        </Section>
        <Section title="Host" tip="The host holds the secret object and answers each guess.">
            <div class="fa-list">
                <div class="fa-item">
                    <Named name="Model" note="The model that picks the object and answers the guesses." />
                    <Choice
                        tip="The model the host asks. The models are those on the Models page."
                        value=game(editor, |s| s.host.model.clone()).into()
                        options=model_options.into()
                        set=Callback::new(move |name: String| edit_game(editor, |s| s.host.model = name))
                        id=field_id(&game_path(&["host", "model"]))
                        error=editor.error(game_path(&["host", "model"]))
                    />
                </div>
                <Text
                    editor=editor
                    at=&["host", "object"]
                    name="Secret object"
                    note="Leave it empty and the host picks one."
                    placeholder="The host picks"
                    tip="What the host is thinking of, like a teapot. Empty, the host's model picks something new each game."
                    read=|s| s.host.object.clone()
                    write=|s, text| s.host.object = text
                />
                <Prompt
                    editor=editor
                    at=&["host", "system"]
                    name="System prompt"
                    note="What the host is told before the game."
                    tip="What the host is told before the game. It can pull in prompt parts with {{ name }}, and use {{ object }} for the secret object and {{ players }} for who plays."
                    read=|s| s.host.system.clone()
                    write=|s, text| s.host.system = text
                />
            </div>
            <div class="fa-list">
                <Prompt
                    editor=editor
                    at=&["host", "pick"]
                    name="Pick a secret object"
                    note="What the host is asked when it picks the object."
                    tip="What the host is asked when the secret object is left empty. It can pull in prompt parts with {{ name }}, and use {{ category }} for a category drawn at random."
                    read=|s| s.host.pick.clone()
                    write=|s, text| s.host.pick = text
                />
                <Temperature editor=editor />
                <Prompt
                    editor=editor
                    at=&["host", "categories"]
                    name="Categories"
                    note="One per line. Each pick draws one at random."
                    tip="What the host may pick from, one per line, like an animal. The pick's {{ category }} is one of them, drawn at random. Empty, the pick has none."
                    read=|s| categories_text(&s.host.categories)
                    write=|s, text| s.host.categories = categories_of(&text)
                    class="tq-categories"
                />
            </div>
        </Section>
        <Section title="Strategies" tip="What the players are told before the game, and the model each strategy asks.">
            <StrategyList
                editor=editor
                of=()
                models=model_options
                name_tip="The name seats are given this strategy by."
                remove_tip="Remove this strategy. Seats that play it need another."
                system_tip="What each player is told before the game. It can pull in prompt parts with {{ name }}, and use {{ name }}, {{ others }} and {{ guesses }} for the player it is playing."
                class="tq-strategies"
            />
        </Section>
        <Section title="Who plays what" tip="Which strategy each seat plays. Seats not split off play the strategy chosen for everyone.">
            <WhoPlaysWhat editor=editor />
        </Section>
        <Section title="Prompt parts" tip="Named pieces of text that prompts pull in with {{ name }}, like the rules, so the host and the players can share them." note=pulled.into_any()>
            <PromptParts editor=editor of=() />
        </Section>
    }
}

/// A count as a stepper: − and + and a field between, from 1 to `most`.
#[component]
fn Stepper(
    /// The count.
    count: Memo<usize>,
    /// The most it can be.
    most: usize,
    /// The field's id.
    id: String,
    /// What is counted, one and many: `player` and `players`.
    called: (&'static str, &'static str),
    /// What the field is.
    tip: String,
    /// Hears the count moved by one, or typed.
    set: Callback<usize>,
) -> impl IntoView {
    let (one, many) = called;
    let typed = move |event| {
        if let Ok(count) = event_target_value(&event).trim().parse::<usize>() {
            set.run(count);
        }
    };
    view! {
        <div class="fa-stepper">
            <button
                aria-label=format!("Fewer {many}")
                data-tip=format!("One {one} fewer.")
                on:click=move |_| set.run(stepped(count.get_untracked(), -1, most))
                disabled=move || { count.get() <= 1 }
            >
                "−"
            </button>
            <input
                type="number"
                min="1"
                max=most.to_string()
                id=id
                aria-label=label(many)
                data-tip=tip
                prop:value=move || count.get().to_string()
                on:input=typed
            />
            <button
                aria-label=format!("More {many}")
                data-tip=format!("One {one} more.")
                on:click=move |_| set.run(stepped(count.get_untracked(), 1, most))
                disabled=move || { count.get() >= most }
            >
                "+"
            </button>
        </div>
    }
}

/// How many play, from 1 to 12, and their names.
#[component]
fn Players(editor: Editor<AppSettings>) -> impl IntoView {
    let count = game(editor, |s| s.players);
    let names = game(editor, |s| in_prose(s.names()));
    let at = game_path(&["players"]);
    let most = NAMES.len();
    view! {
        <div class="fa-item">
            <Named name="Players" note="Named in seat order, Alice to Leo." />
            <Stepper
                count=count
                most=most
                id=field_id(&at)
                called=("player", "players")
                tip=format!("How many players, from 1 to {most}. Type a number or use − and +.")
                set=Callback::new(move |players| edit_game(editor, |s| s.players = players))
            />
            <p class="fa-note tq-names" data-tip="The players' names, which follow from how many play.">
                {move || names.get()}
            </p>
            <Problem error=editor.error(at) />
        </div>
    }
}

/// How many guesses the players share, from 1 to 20.
#[component]
fn Guesses(editor: Editor<AppSettings>) -> impl IntoView {
    let count = game(editor, |s| s.guesses as usize);
    let at = game_path(&["guesses"]);
    let most = GUESSES as usize;
    view! {
        <div class="fa-item">
            <Named name="Guesses" note="Shared by the players. When they run out, the host wins." />
            <Stepper
                count=count
                most=most
                id=field_id(&at)
                called=("guess", "guesses")
                tip=format!("How many guesses the players share, from 1 to {GUESSES}. Type a number or use − and +.")
                set=Callback::new(move |guesses: usize| edit_game(editor, |s| s.guesses = guesses as u64))
            />
            <Problem error=editor.error(at) />
        </div>
    }
}

/// A row with a one-line field for the game's setting at `at`.
#[component]
fn Text(
    editor: Editor<AppSettings>,
    at: &'static [&'static str],
    name: &'static str,
    note: &'static str,
    placeholder: &'static str,
    tip: &'static str,
    read: fn(&GameSettings) -> String,
    write: fn(&mut GameSettings, String),
) -> impl IntoView {
    let value = game(editor, read);
    let at = game_path(at);
    let id = field_id(&at);
    let error = editor.error(at);
    view! {
        <div class="fa-item tq-text-field">
            <Named name=name note=note field=id.clone() />
            <input
                type="text"
                id=id
                spellcheck="false"
                autocomplete="off"
                placeholder=placeholder
                data-tip=tip
                class:fa-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=move |event| {
                    let text = event_target_value(&event);
                    edit_game(editor, |s| write(s, text));
                }
            />
            <Problem error=error />
        </div>
    }
}

/// A row with a prompt for the game's setting at `at`, monospace, under
/// its name.
#[component]
fn Prompt(
    editor: Editor<AppSettings>,
    at: &'static [&'static str],
    name: &'static str,
    note: &'static str,
    tip: &'static str,
    read: fn(&GameSettings) -> String,
    write: fn(&mut GameSettings, String),
    #[prop(optional)] class: &'static str,
) -> impl IntoView {
    let value = game(editor, read);
    let at = game_path(at);
    let id = field_id(&at);
    let error = editor.error(at);
    view! {
        <div class="fa-item tq-prompt">
            <Named name=name note=note field=id.clone() />
            <textarea
                class=format!("fa-mono {class}")
                id=id
                spellcheck="false"
                data-tip=tip
                class:fa-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=move |event| {
                    let text = event_target_value(&event);
                    edit_game(editor, |s| write(s, text));
                }
            ></textarea>
            <Problem error=error />
        </div>
    }
}

/// The temperature the host picks at, from 0 to 2.
#[component]
fn Temperature(editor: Editor<AppSettings>) -> impl IntoView {
    let value = game(editor, |s| s.host.temperature.to_string());
    let at = game_path(&["host", "temperature"]);
    let id = field_id(&at);
    let error = editor.error(at);
    view! {
        <div class="fa-item tq-text-field">
            <Named name="Temperature" note="Higher picks more varied objects." field=id.clone() />
            <input
                type="number"
                min="0"
                max="2"
                step="0.1"
                id=id
                data-tip="How freely the host picks, from 0 to 2. Higher, it picks less often the same few things."
                class:fa-invalid=move || error.with(Option::is_some)
                prop:value=move || value.get()
                on:input=move |event| {
                    if let Ok(temperature) = event_target_value(&event).trim().parse::<f64>() {
                        edit_game(editor, |s| s.host.temperature = temperature);
                    }
                }
            />
            <Problem error=error />
        </div>
    }
}

/// Who plays what: the strategy everyone plays and, behind **Split
/// seats…**, how many seats play each strategy instead.
#[component]
fn WhoPlaysWhat(editor: Editor<AppSettings>) -> impl IntoView {
    let everyone = game(editor, |s| s.strategy.clone());
    // Every strategy, and any name given seats that isn't one, so that its
    // error can be shown.
    let named = game(editor, |s| {
        let mut names: Vec<String> = s.strategies.keys().cloned().collect();
        names.extend(
            s.allotments
                .keys()
                .filter(|name| !s.strategies.contains_key(*name))
                .cloned(),
        );
        names
    });
    let options = Memo::new(move |_| {
        let mut options = vec![(String::new(), "Choose a strategy".to_string())];
        options.extend(editor.settings.with(|s| {
            s.game
                .strategies
                .keys()
                .map(|name| (name.clone(), name.clone()))
                .collect::<Vec<_>>()
        }));
        options
    });
    let allotments_at = game_path(&["allotments"]);
    let split_id = field_id(&allotments_at);
    let split = RwSignal::new(
        editor
            .settings
            .with_untracked(|s| !s.game.allotments.is_empty()),
    );
    view! {
        <div class="fa-list">
            <div class="fa-item">
                <Named name="Everyone" note="Every seat plays this unless it is split off." />
                <button
                    class="fa-link"
                    aria-expanded=move || split.get().to_string()
                    data-tip="Give some seats other strategies. The seats left play the strategy chosen for everyone."
                    on:click=move |_| split.update(|split| *split = !*split)
                >
                    "Split seats…"
                </button>
                <Choice
                    tip="The strategy every seat plays unless it is split off."
                    value=everyone.into()
                    options=options.into()
                    set=Callback::new(move |name: String| edit_game(editor, |s| s.strategy = name))
                    id=field_id(&game_path(&["strategy"]))
                    error=editor.error(game_path(&["strategy"]))
                />
                <Show when=move || split.get()>
                    <div class="tq-split" id=split_id.clone() tabindex="-1">
                        <For each=move || named.get() key=Clone::clone let:name>
                            <Seats editor=editor name=name />
                        </For>
                    </div>
                </Show>
                <Problem error=editor.error(allotments_at) />
            </div>
        </div>
    }
}

/// How many seats play the strategy `name` instead of everyone's.
#[component]
fn Seats(editor: Editor<AppSettings>, name: String) -> impl IntoView {
    let read = name.clone();
    let seats = game(editor, move |s| {
        s.allotments.get(&read).copied().unwrap_or_default()
    });
    let at = game_path(&["allotments", &name]);
    let written = name.clone();
    let set = move |event| {
        let seats = event_target_value(&event)
            .trim()
            .parse::<usize>()
            .unwrap_or_default();
        let name = written.clone();
        edit_game(editor, |s| {
            if seats == 0 {
                s.allotments.remove(&name);
            } else {
                s.allotments.insert(name, seats);
            }
        });
    };
    view! {
        <label class="tq-seats" data-tip=format!("How many seats play {name} instead. 0 gives it none.")>
            <input type="number" min="0" id=field_id(&at) prop:value=move || seats.get().to_string() on:change=set />
            <span>{name.clone()}</span>
        </label>
        <Problem error=editor.error(at) />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_players_names_are_a_list_in_prose() {
        assert_eq!(in_prose(&["Alice", "Bob", "Carol"]), "Alice, Bob and Carol");
        assert_eq!(in_prose(&["Alice"]), "Alice");
        assert_eq!(in_prose(&[]), "");
    }

    #[test]
    fn the_categories_read_back_as_typed() {
        for text in ["an animal\na tool", "an animal\n", ""] {
            assert_eq!(categories_text(&categories_of(text)), text);
        }
        assert_eq!(categories_of("a\n\nb"), ["a", "", "b"]);
    }

    #[test]
    fn a_count_steps_by_one_from_one_to_the_most() {
        assert_eq!(stepped(4, 1, 12), 5);
        assert_eq!(stepped(1, -1, 12), 1);
        assert_eq!(stepped(12, 1, 12), 12);
        assert_eq!(stepped(40, -1, 20), 20);
    }
}
