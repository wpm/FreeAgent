//! The page: a header holding the secret object, the outcome and the
//! guesses used with the time left, over the chat in a readable column.
//! Every part draws the [`Screen`], whose cues come from a game playing
//! live in the desktop app.

use std::time::Duration;

use free_agent_gui::clock::CountdownClock;
use free_agent_gui::style::FreeAgentStyle;
use leptos::prelude::*;
use leptos_rich_chat::{Message, RenderOptions, RichText, Transcript};
use twenty_questions_gui_types::Cue;

use crate::desktop::Desktop;
use crate::screen::{Body, NARRATOR, Pip, Post, Screen, THINKING_DELAY, Thinker, dots_due, ending};
use crate::settings::SettingsWindow;

/// How often the clock redraws.
const TICK: Duration = Duration::from_millis(100);

/// The page: the Settings window at `#settings`, and otherwise the main
/// window.
#[component]
pub fn App() -> impl IntoView {
    let hash = window().location().hash().unwrap_or_default();
    if hash.starts_with("#settings") {
        view! { <SettingsWindow /> }.into_any()
    } else {
        view! { <Desktop /> }.into_any()
    }
}

/// The screen as cues arrive one at a time, and the viewer's time when the
/// latest arrived.
#[derive(Clone, Copy)]
pub struct Live {
    /// What the screen shows.
    pub screen: RwSignal<Screen>,
    /// The viewer's time when the latest cue arrived.
    arrived: StoredValue<f64>,
}

impl Default for Live {
    /// An empty screen.
    fn default() -> Self {
        Self {
            screen: RwSignal::new(Screen::default()),
            arrived: StoredValue::new(elapsed()),
        }
    }
}

impl Live {
    /// The viewer's time.
    fn now(self) -> f64 {
        elapsed()
    }

    /// Clear the screen for a new game. What the last game's cues left to
    /// do later, they leave undone.
    pub fn reset(self) {
        self.screen.update(Screen::reset);
    }

    /// Show `cue` as it arrives: the clock runs on to it, and the dots of a
    /// call it sends out show once the call has been out a while, unless
    /// the screen is reset first.
    pub fn show(self, cue: &Cue) {
        let screen = self.screen;
        let at = self.now();
        let waited = at - self.arrived.get_value();
        self.arrived.set_value(at);
        screen.update(|screen| {
            screen.advance(waited);
            screen.apply(cue);
        });
        if let Some(due) = dots_due(cue) {
            let era = screen.with_untracked(Screen::era);
            let rate = screen.with_untracked(|screen| screen.clock.rate);
            let delay = Duration::from_millis(THINKING_DELAY).div_f64(rate);
            let reveal = move || screen.update(|screen| screen.within(era, |s| s.reveal(due)));
            set_timeout(reveal, delay);
        }
    }
}

/// The game on `live`'s screen: a header with the game's name, the secret
/// object, how the game ended, the `children`, which control it, and the
/// guesses used over the time left; then the chat. A link goes to
/// `on_link`.
#[component]
pub(crate) fn Viewer(
    live: Live,
    #[prop(into)] on_link: Callback<String>,
    children: Children,
) -> impl IntoView {
    let screen = live.screen;
    // The viewer's time, now.
    let tick = RwSignal::new(live.now());
    if let Ok(ticking) = set_interval_with_handle(move || tick.set(live.now()), TICK) {
        on_cleanup(move || ticking.clear());
    }
    // A memo, so that a stopped clock redraws nothing as it ticks.
    let now = Signal::from(Memo::new(move |_| {
        let waited = tick.get() - live.arrived.get_value();
        screen.with(|screen| screen.clock.now(waited))
    }));

    let options = RenderOptions {
        math: false,
        highlight: false,
        images: false,
        ..RenderOptions::default()
    };
    // Memos, so that a cue that leaves a part as it was redraws none of it.
    let names = Memo::new(move |_| screen.with(|screen| screen.names.clone()));
    let posts = Memo::new(move |_| screen.with(|screen| screen.posts.clone()));
    let thinkers = Memo::new(move |_| screen.with(|screen| screen.thinkers.clone()));
    let won = Memo::new(move |_| screen.with(|screen| screen.won));
    let secret = Memo::new(move |_| screen.with(|screen| screen.secret.clone()));
    let outcome = Memo::new(move |_| {
        screen.with(|screen| {
            let outcome = screen.outcome.as_ref()?;
            Some(ending(outcome, screen.won))
        })
    });
    let timer = Memo::new(move |_| screen.with(Screen::timer));
    let pips = Memo::new(move |_| screen.with(Screen::pips));
    let used = Memo::new(move |_| screen.with(Screen::used));
    let guesses = Memo::new(move |_| screen.with(|screen| screen.guesses));
    // Every thinker's dots share one text, which stays empty.
    let silence = RwSignal::new(String::new());
    // The line over a player's bubbles: their square and name.
    let sender = Callback::new(move |name: String| {
        let player = screen.with_untracked(|screen| screen.color(&name).is_some());
        view! {
            {player.then(|| view! { <span class="tq-square" aria-hidden="true"></span> })}
            <span>{name}</span>
        }
        .into_any()
    });
    let rendered = options.clone();
    let messages = Signal::derive(move || {
        let mut messages = posts.with(|posts| {
            screen.with_untracked(|screen| {
                posts
                    .iter()
                    .map(|post| message(post, screen, won, rendered.clone(), on_link))
                    .collect::<Vec<_>>()
            })
        });
        thinkers.with(|thinkers| {
            messages.extend(thinkers.iter().map(|thinker| dots(thinker, silence)));
        });
        messages
    });

    view! {
        <FreeAgentStyle names=names narrator=NARRATOR />
        <div class="fa-app tq-app">
            <header class="fa-header fa-chrome tq-header">
                <div class="tq-title">
                    <h1>"Twenty Questions"</h1>
                    <p class="tq-secret" data-tip="What the host is thinking of. The players never see it.">
                        {move || match secret.get() {
                            Some(object) => view! { "Secret: " <strong>{object}</strong> }.into_any(),
                            None => view! { <span class="tq-picking">"The host is choosing a secret…"</span> }.into_any(),
                        }}
                    </p>
                    {move || outcome.get().map(|outcome| view! { <p class="tq-outcome">{outcome}</p> })}
                </div>
                <div class="tq-controls">{children()}</div>
                <div class="tq-count" role="group" aria-label="Guesses used">
                    <p class="tq-tally" data-tip="The guesses the players have put, of those they have.">
                        <span class="tq-used">{move || used.get()}</span>
                        <span class="tq-of">{move || format!(" / {}", guesses.get())}</span>
                    </p>
                    <div class="tq-pips" aria-hidden="true">
                        {move || pips.get().into_iter().map(pip).collect_view()}
                    </div>
                    {move || timer.get().map(|timer| view! { <CountdownClock timer=timer now=now /> })}
                </div>
            </header>
            <main class="tq-main">
                <Transcript
                    messages=messages
                    show_names=true
                    sender=sender
                    options=options
                    on_link=on_link
                />
            </main>
        </div>
    }
}

/// The viewer's time in milliseconds, from the page's own clock.
fn elapsed() -> f64 {
    window()
        .performance()
        .map_or(0.0, |performance| performance.now())
}

/// One of the header's pips.
fn pip(pip: Pip) -> impl IntoView {
    let (class, style) = match pip {
        Pip::Open => ("tq-pip", String::new()),
        Pip::Waiting => ("tq-pip tq-waiting", String::new()),
        Pip::Used => ("tq-pip tq-used-pip", String::new()),
        Pip::Won(color) => (
            "tq-pip tq-won-pip",
            format!("--tq-mark: var(--tq-player-{}-mark)", color + 1),
        ),
    };
    view! { <span class=class style=style></span> }
}

/// `post` as a Rich-Chat message. A guess and an answer carry their tag,
/// and the answer the guess it answers; each is ringed once `won` names
/// its guess, in the guesser's color.
fn message(
    post: &Post,
    screen: &Screen,
    won: Memo<Option<u64>>,
    options: RenderOptions,
    on_link: Callback<String>,
) -> Message {
    let (id, name) = (post.id.clone(), post.name.clone());
    match &post.body {
        Body::Told(text) | Body::Said(text) => Message::new(id, name, text.clone()),
        Body::Guess { guess, text } => {
            let (guess, text) = (*guess, text.clone());
            Message::view(id, name, move || {
                view! {
                    <div class="rc-bubble tq-guess" class:tq-won=move || won.get() == Some(guess)>
                        <p class="tq-tag">{format!("Guess {guess}")}</p>
                        <RichText content=text.clone() options=options.clone() on_link=on_link />
                    </div>
                }
            })
        }
        Body::Answer { guess, asked, text } => {
            let (guess, asked, text) = (*guess, asked.clone(), text.clone());
            let ring = screen
                .asker_color(guess)
                .map(|color| format!("--tq-ring: var(--tq-player-{}-mark)", color + 1))
                .unwrap_or_default();
            Message::view(id, name, move || {
                let quote =
                    (!asked.is_empty()).then(|| view! { <p class="tq-quote">{asked.clone()}</p> });
                view! {
                    <div class="rc-bubble tq-answer" style=ring.clone() class:tq-won=move || won.get() == Some(guess)>
                        <p class="tq-tag">{format!("Answer {guess}")}</p>
                        {quote}
                        <RichText content=text.clone() options=options.clone() on_link=on_link />
                    </div>
                }
            })
        }
        Body::Uncounted(text) => {
            let text = text.clone();
            Message::view(id, name, move || {
                view! {
                    <div class="rc-bubble tq-uncounted">
                        <p class="tq-tag">"Too late to count"</p>
                        <RichText content=text.clone() options=options.clone() on_link=on_link />
                    </div>
                }
            })
        }
    }
}

/// `thinker`'s dots, `think-Alice`: a live message whose text, `silence`,
/// is empty.
fn dots(thinker: &Thinker, silence: RwSignal<String>) -> Message {
    let id = format!("think-{}", thinker.who);
    Message::streaming(id, thinker.who.clone(), silence)
}
