//! The Game page: how many play, the host's model and its secret object,
//! and the strategy the players play, each a section of grouped lists.

use free_agent_gui::settings::strategies::{StrategyCard, path_of};
use free_agent_gui::settings::{Choice, Editor, Named, Page, PageHead, Problem, Section, field_id};
use leptos::prelude::*;
use leptos::task::spawn_local;
use twenty_questions_gui_types::settings::{AppSettings, FieldError, GameSettings};

use super::GamePage;
use crate::tauri;

/// The most players a table seats: Alice to Leo.
const MOST: usize = 12;

/// The path of the field `rest` of the game's settings.
fn game_path(rest: &[&str]) -> Vec<String> {
    path_of::<AppSettings>((), rest)
}

/// Whether the page shows the error at `path` beside its own field.
fn shown_beside(path: &[String]) -> bool {
    match path {
        [game, rest @ ..] if game == "game" => matches!(
            rest.iter().map(String::as_str).collect::<Vec<_>>()[..],
            ["players"]
                | ["host", "model"]
                | ["host", "object"]
                | ["strategies", _, "model" | "system"]
        ),
        _ => true,
    }
}

/// The game's errors that no field on the page shows, one per line.
fn elsewhere(errors: &[FieldError]) -> Option<String> {
    let lines: Vec<&str> = errors
        .iter()
        .filter(|error| !shown_beside(&error.path))
        .map(|error| error.message.as_str())
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
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

/// The Game page: the players, the host and the strategies, with
/// **Restore defaults…**.
#[component]
pub(super) fn GameTab(editor: Editor<AppSettings>) -> impl IntoView {
    let confirming = RwSignal::new(false);
    let restore = move |_| {
        confirming.set(false);
        spawn_local(async move {
            if let Ok(defaults) = tauri::game_defaults().await {
                edit_game(editor, |settings| *settings = defaults);
            }
        });
    };
    let strategy_names = game(editor, |s| s.strategies.keys().cloned().collect::<Vec<_>>());
    let model_options = Memo::new(move |_| {
        let mut names: Vec<(String, String)> = vec![(String::new(), "Choose a model".to_string())];
        editor.settings.with(|s| {
            names.extend(s.models.keys().map(|name| (name.clone(), name.clone())));
        });
        names
    });
    let others = Signal::derive(move || editor.errors.with(|errors| elsewhere(errors)));
    view! {
        <PageHead title="Game" tip=GamePage.tip()>
            <Show
                when=move || confirming.get()
                fallback=move || view! {
                    <button
                        on:click=move |_| confirming.set(true)
                        data-tip="Put back the game settings the app came with. The models stay as they are. You confirm first."
                    >
                        "Restore defaults…"
                    </button>
                }
            >
                <div class="tq-confirm" role="alertdialog">
                    <span>"Replace the game settings with the defaults? The models stay as they are."</span>
                    <button on:click=restore data-tip="Replace the game settings with the defaults.">"Restore"</button>
                    <button on:click=move |_| confirming.set(false) data-tip="Keep the settings as they are.">"Cancel"</button>
                </div>
            </Show>
        </PageHead>
        <span id=field_id(&game_path(&[])) tabindex="-1"></span>
        <Problem error=others />
        <Section title="Players" tip="The model players at the table, Alice to Leo, each in a color of their own.">
            <div class="fa-list">
                <Players editor=editor />
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
                <Secret editor=editor />
            </div>
        </Section>
        <Section title="Strategy" tip="What the players are told before the game, and the model they ask.">
            <For each=move || strategy_names.get() key=Clone::clone let:name>
                <StrategyCard
                    editor=editor
                    of=()
                    name=name
                    models=model_options
                    name_tip="The strategy every player plays."
                    remove_tip="Remove this strategy. The players need one to play, which Restore defaults brings back."
                    system_tip="What each player is told before the game. It can pull in prompt parts with {{ name }}, and use {{ name }}, {{ others }} and {{ guesses }} for the player it is playing."
                />
            </For>
        </Section>
    }
}

/// How many play: a stepper from 1 to 12.
#[component]
fn Players(editor: Editor<AppSettings>) -> impl IntoView {
    let count = game(editor, |s| s.players);
    let at = game_path(&["players"]);
    let step = move |by: isize| {
        edit_game(editor, |s| {
            s.players = s.players.saturating_add_signed(by).clamp(1, MOST);
        });
    };
    let typed = move |event| {
        if let Ok(count) = event_target_value(&event).trim().parse::<usize>() {
            edit_game(editor, |s| s.players = count);
        }
    };
    view! {
        <div class="fa-item">
            <Named name="Players" note="Alice, Bob, Carol and on, up to Leo." />
            <div class="fa-stepper">
                <button
                    aria-label="Fewer players"
                    data-tip="One player fewer."
                    on:click=move |_| step(-1)
                    disabled=move || count.get() <= 1
                >
                    "−"
                </button>
                <input
                    type="number"
                    min="1"
                    max=MOST.to_string()
                    id=field_id(&at)
                    aria-label="Players"
                    data-tip="How many players, from 1 to 12. Type a number or use − and +."
                    prop:value=move || count.get().to_string()
                    on:input=typed
                />
                <button
                    aria-label="More players"
                    data-tip="One player more."
                    on:click=move |_| step(1)
                    disabled=move || count.get() >= MOST
                >
                    "+"
                </button>
            </div>
            <Problem error=editor.error(at) />
        </div>
    }
}

/// The secret object: a field, empty for the host to pick one.
#[component]
fn Secret(editor: Editor<AppSettings>) -> impl IntoView {
    let object = game(editor, |s| s.host.object.clone());
    let at = game_path(&["host", "object"]);
    let id = field_id(&at);
    let error = editor.error(at);
    view! {
        <div class="fa-item tq-secret-field">
            <Named name="Secret object" note="Leave it empty and the host picks one." field=id.clone() />
            <input
                type="text"
                id=id
                spellcheck="false"
                autocomplete="off"
                placeholder="The host picks"
                data-tip="What the host is thinking of, like a teapot. Empty, the host's model picks something new each game."
                class:fa-invalid=move || error.with(Option::is_some)
                prop:value=move || object.get()
                on:input=move |event| {
                    let object = event_target_value(&event);
                    edit_game(editor, |s| s.host.object = object);
                }
            />
            <Problem error=error />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_the_page_has_no_field_for_show_at_its_head() {
        let errors = [
            FieldError::new(&["game", "players"], "a"),
            FieldError::new(&["game", "host", "model"], "b"),
            FieldError::new(&["game", "host", "object"], "c"),
            FieldError::new(&["game", "strategies", "everyone", "system"], "d"),
            FieldError::new(&["game", "strategies", "everyone", "model"], "e"),
            FieldError::new(&["models", "qwen", "id"], "f"),
            FieldError::new(&["game", "time_limit"], "g"),
            FieldError::new(&["game"], "h"),
        ];
        assert_eq!(elsewhere(&errors), Some("g\nh".to_string()));
        assert_eq!(elsewhere(&errors[..6]), None);
    }
}
