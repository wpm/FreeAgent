//! The page: a header holding the secret object, the outcome and the
//! guesses used with the time left, over the chat in a readable column.
//! Every part draws the [`Screen`]. In the browser the cues come from a
//! recorded game; in the desktop app, from a game playing live.

use std::time::Duration;

use free_agent_gui::clock::CountdownClock;
use free_agent_gui::review::ReviewHeader;
use free_agent_gui::style::FreeAgentStyle;
use free_agent_gui::tauri::in_tauri;
use free_agent_gui::tip::Tips;
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_rich_chat::{Message, RenderOptions, RichText, Transcript};
use twenty_questions_gui_types::Cue;
use twenty_questions_gui_types::go::Go;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::address::Address;
use crate::desktop::Desktop;
use crate::review::Review;
use crate::screen::{
    self, Body, NARRATOR, Pip, Post, Screen, THINKING_DELAY, Thinker, dots_due, ending,
};
use crate::settings::SettingsWindow;

/// How often **Play** shows the next cue.
const STEP: Duration = Duration::from_millis(250);

/// How often the clock redraws.
const TICK: Duration = Duration::from_millis(100);

/// The page: the desktop app's main window inside the desktop app, its
/// Settings window at `#settings`, and the web page in a browser.
#[component]
pub fn App() -> impl IntoView {
    let hash = window().location().hash().unwrap_or_default();
    if in_tauri() && hash.starts_with("#settings") {
        view! { <SettingsWindow /> }.into_any()
    } else if in_tauri() {
        view! { <Desktop /> }.into_any()
    } else {
        view! { <Web /> }.into_any()
    }
}

/// The web page: it fetches the cues of the recorded game the URL hash
/// names, `#prompted` by default, from `game/` beside the page and shows
/// the whole game at once, in review on its last bubble. **Play** clears
/// the screen and plays the cues again, one every 250 ms, or ten times as
/// fast for a hash that ends in `-fast`, and the game is back in review
/// once they have all been shown.
#[component]
fn Web() -> impl IntoView {
    let live = Live::default();
    let reviewing = Reviewing::new(live);
    let reviewed = Memo::new(move |_| reviewing.review.with(Option::is_some));
    let cues = StoredValue::new(Vec::<Cue>::new());
    let failure = RwSignal::new(None::<String>);
    let played = RwSignal::new(false);
    let playing = StoredValue::new(None::<IntervalHandle>);
    // The cue Play shows next. It belongs to the page, since Play leaves
    // the review it was pressed in.
    let next = StoredValue::new(0usize);
    let address = RwSignal::new(hashed());

    let stop = move || {
        if let Some(handle) = playing.get_value() {
            handle.clear();
        }
    };
    // The game at rest, in review on its last bubble.
    let rest = StoredValue::new(None::<Review>);
    let review = move || {
        if let Some(review) = rest.get_value() {
            reviewing.open(review);
        }
    };

    let _ = window_event_listener(ev::hashchange, move |_| address.set(hashed()));
    Effect::new(move |_| {
        let name = address.with(|address| address.game.clone());
        stop();
        reviewing.close();
        rest.set_value(None);
        played.set(false);
        failure.set(None);
        spawn_local(async move {
            let fetched = fetched(&name).await;
            if address.with_untracked(|address| address.game != name) {
                return;
            }
            match fetched {
                Ok(fetched) => {
                    rest.set_value(Review::played(&fetched));
                    cues.set_value(fetched);
                    review();
                }
                Err(error) => failure.set(Some(error)),
            }
        });
    });

    let play = move |_| {
        stop();
        reviewing.close();
        let pace = address.with_untracked(|address| address.pace);
        live.set_pace(pace);
        played.set(true);
        next.set_value(0);
        let handle = set_interval_with_handle(
            move || {
                let index = next.get_value();
                let Some(cue) = cues.with_value(|cues| cues.get(index).cloned()) else {
                    stop();
                    review();
                    return;
                };
                next.set_value(index + 1);
                live.show(&cue);
            },
            STEP.div_f64(pace),
        );
        playing.set_value(handle.ok());
    };

    let controls = move || {
        view! {
            <button class="fa-primary" on:click=play data-tip="Play the game again from the start, a moment at a time.">
                {move || if played.get() { "Play again" } else { "Play" }}
            </button>
        }
    };
    view! {
        <Tips />
        <Show
            when=move || !reviewed.get()
            fallback=move || view! { <Reviewer reviewing=reviewing>{controls()}</Reviewer> }
        >
            <Viewer live=live failure=failure>{controls()}</Viewer>
        </Show>
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
    /// How many times the usual speed the cues arrive at.
    pace: StoredValue<f64>,
}

impl Default for Live {
    /// An empty screen.
    fn default() -> Self {
        Self {
            screen: RwSignal::new(Screen::default()),
            arrived: StoredValue::new(elapsed()),
            pace: StoredValue::new(1.0),
        }
    }
}

impl Live {
    /// The viewer's time, run at the pace the cues arrive at.
    fn now(self) -> f64 {
        elapsed() * self.pace.get_value()
    }

    /// Have the cues arrive at `pace` times the usual speed.
    pub fn set_pace(self, pace: f64) {
        self.pace.set_value(pace);
        self.arrived.set_value(self.now());
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
            let delay = Duration::from_millis(THINKING_DELAY)
                .div_f64(rate)
                .div_f64(self.pace.get_value());
            let reveal = move || screen.update(|screen| screen.within(era, |s| s.reveal(due)));
            set_timeout(reveal, delay);
        }
    }
}

/// A game log under review on `live`'s screen, or none: what the main
/// window's keys, the **Go** menu and a click on a bubble move.
#[derive(Clone, Copy)]
pub struct Reviewing {
    /// The log, while one is under review.
    pub review: RwSignal<Option<Review>>,
    /// The screen it shows on.
    live: Live,
}

impl Reviewing {
    /// No log under review, to show on `live`'s screen.
    pub fn new(live: Live) -> Self {
        Self {
            review: RwSignal::new(None),
            live,
        }
    }

    /// Review `review`, on the bubble its cursor is on. What the screen
    /// showed before, it leaves undone.
    pub fn open(self, review: Review) {
        self.live.reset();
        self.review.set(Some(review));
        self.shown();
    }

    /// Stop reviewing, and clear the screen.
    pub fn close(self) {
        self.review.set(None);
        self.live.reset();
    }

    /// Move the cursor as `go` says.
    pub fn go(self, go: Go) {
        self.moved(|review| review.go(go));
    }

    /// Put the cursor on the bubble whose message ID is `id`.
    pub fn select(self, id: &str) {
        if let Some(k) = screen::bubble(id) {
            self.moved(|review| review.select(k));
        }
    }

    /// Show game `game` on its first bubble.
    pub fn choose(self, game: usize) {
        self.moved(|review| review.choose(game));
    }

    /// Move the cursor by `act`, and show the bubble it lands on. Another
    /// game shows on a screen of its own.
    fn moved(self, act: impl FnOnce(&mut Review)) {
        let moved = self.review.try_update(|review| {
            review.as_mut().map(|review| {
                let game = review.game();
                act(review);
                review.game() != game
            })
        });
        if let Some(Some(another)) = moved {
            if another {
                self.live.reset();
            }
            self.shown();
        }
    }

    /// Show the bubble under the cursor.
    fn shown(self) {
        if let Some(shown) =
            (self.review).with_untracked(|review| review.as_ref().map(Review::screen))
        {
            self.live.screen.update(|screen| screen.review(shown));
        }
    }
}

/// The game under review on `reviewing`'s screen: the viewer, its header
/// naming the file of a log, the game with ‹ › and a list of the log's
/// games when it holds more than one, where the cursor is, and the
/// `children`, which control it. Unmodified arrows and Home and End move
/// the cursor, and a click on a bubble puts it there.
#[component]
pub(crate) fn Reviewer(
    reviewing: Reviewing,
    #[prop(into, optional_no_strip)] on_link: Option<Callback<String>>,
    children: Children,
) -> impl IntoView {
    let review = reviewing.review;
    // Memos, so that moving the cursor redraws none of what names the log.
    let warning = Memo::new(move |_| {
        review.with(|review| {
            let problems = review.as_ref()?.problems();
            (!problems.is_empty())
                .then(|| format!("Some of this log couldn't be read: {}", problems.join(" ")))
        })
    });
    let name =
        Memo::new(move |_| review.with(|review| review.as_ref()?.name().map(str::to_string)));
    let games = Memo::new(move |_| review.with(|review| review.as_ref().map_or(0, Review::games)));
    let game = Memo::new(move |_| review.with(|review| review.as_ref().map_or(0, Review::game)));
    let titles = Memo::new(move |_| {
        review.with(|review| review.as_ref().map(Review::titles).unwrap_or_default())
    });
    let position =
        Signal::derive(move || review.with(|review| review.as_ref().map(Review::position)));
    let on_select = Callback::new(move |id: String| reviewing.select(&id));
    view! {
        <Viewer live=reviewing.live warning=warning on_link=on_link on_select=on_select>
            <ReviewHeader
                name=name
                games=games
                game=game
                titles=titles
                position=position
                on_go=Callback::new(move |go| reviewing.go(go))
                on_choose=Callback::new(move |game| reviewing.choose(game))
            >
                {children()}
            </ReviewHeader>
        </Viewer>
    }
}

/// The game on `live`'s screen: a header with the game's name, the secret
/// object, how the game ended, the `children`, which control it, and the
/// guesses used over the time left; then the chat. A `failure` or a
/// `warning` shows above the chat, and a link goes to `on_link` when there
/// is one. The bubble under the screen's cursor is selected, and a bubble
/// clicked goes to `on_select`.
#[component]
pub(crate) fn Viewer(
    live: Live,
    #[prop(optional, into)] failure: Signal<Option<String>>,
    #[prop(optional, into)] warning: Signal<Option<String>>,
    #[prop(into, optional_no_strip)] on_link: Option<Callback<String>>,
    #[prop(optional)] on_select: Option<Callback<String>>,
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
    let posts = Memo::new(move |_| screen.with(|screen| (screen.era(), screen.posts.clone())));
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
    // A guess waiting for its answer pulses while the cues arrive, and
    // holds still at rest.
    let running = Memo::new(move |_| screen.with(|screen| screen.clock.running));
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
    let selected = Signal::derive(move || {
        screen.with(|screen| {
            let post = screen.posts.get(screen.cursor?)?;
            Some(screen::key(screen.era(), &post.id))
        })
    });
    let rendered = options.clone();
    let messages = Signal::derive(move || {
        let mut messages = posts.with(|(era, posts)| {
            screen.with_untracked(|screen| {
                (posts.iter())
                    .map(|post| message(*era, post, screen, won, rendered.clone(), on_link))
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
                <div class="tq-count" role="group" aria-label="Guesses used">
                    <p class="tq-tally" data-tip="The guesses the players have put, of those they have.">
                        <span class="tq-used">{move || used.get()}</span>
                        <span class="tq-of">{move || format!(" / {}", guesses.get())}</span>
                    </p>
                    <div class="tq-pips" class:tq-still=move || !running.get() aria-hidden="true">
                        {move || pips.get().into_iter().map(pip).collect_view()}
                    </div>
                    {move || timer.get().map(|timer| view! { <CountdownClock timer=timer now=now /> })}
                </div>
                <div class="tq-controls">{children()}</div>
            </header>
            <main class="tq-main">
                {move || failure.get().map(|error| view! { <p class="fa-error">{error}</p> })}
                {move || warning.get().map(|warning| view! { <p class="fa-warning fa-review-warning"><span>{warning}</span></p> })}
                <Transcript
                    messages=messages
                    show_names=true
                    sender=sender
                    options=options
                    on_link=on_link
                    selected=selected
                    on_select=on_select
                />
            </main>
        </div>
    }
}

/// The address the URL hash names.
fn hashed() -> Address {
    Address::parse(&window().location().hash().unwrap_or_default())
}

/// The cues of the recorded game `name`, in `game/name.json` beside the
/// page.
async fn fetched(name: &str) -> Result<Vec<Cue>, String> {
    let file = format!("game/{name}.json");
    let failed = |error: JsValue| format!("{file}: {error:?}");
    let response: web_sys::Response = JsFuture::from(window().fetch_with_str(&file))
        .await
        .map_err(failed)?
        .dyn_into()
        .map_err(failed)?;
    if !response.ok() {
        return Err(format!("{file}: {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(failed)?)
        .await
        .map_err(failed)?;
    serde_json::from_str(&text.as_string().unwrap_or_default())
        .map_err(|error| format!("{file}: {error}"))
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
        Pip::Won(color) => ("tq-pip tq-won-pip", format!("--tq-mark: {}", mark(color))),
    };
    view! { <span class=class style=style></span> }
}

/// `post` as a Rich-Chat message, keyed for `era`. A guess, an answer
/// and a guess too late to count carry their tag, and the answer the
/// guess it answers, marked in the guesser's color; a guess and its
/// answer are ringed in it once `won` names the guess.
fn message(
    era: u64,
    post: &Post,
    screen: &Screen,
    won: Memo<Option<u64>>,
    options: RenderOptions,
    on_link: Option<Callback<String>>,
) -> Message {
    let (id, name) = (screen::key(era, &post.id), post.name.clone());
    let (class, tag, guess, asked, text) = match &post.body {
        Body::Told(text) | Body::Said(text) => return Message::new(id, name, text.clone()),
        Body::Guess { guess, text } => (
            "tq-guess",
            format!("Guess {guess}"),
            Some(*guess),
            None,
            text,
        ),
        Body::Answer { guess, asked, text } => {
            let asked = (!asked.is_empty()).then(|| asked.clone());
            (
                "tq-answer",
                format!("Answer {guess}"),
                Some(*guess),
                asked,
                text,
            )
        }
        Body::Uncounted(text) => (
            "tq-uncounted",
            "Too late to count".to_string(),
            None,
            None,
            text,
        ),
    };
    let ring = match &post.body {
        Body::Answer { guess, .. } => screen
            .guesser_color(*guess)
            .map(|color| format!("--tq-ring: {}", mark(color))),
        _ => None,
    };
    let text = text.clone();
    Message::view(id, name, move || {
        let quote = asked
            .clone()
            .map(|asked| view! { <p class="tq-quote">{asked}</p> });
        view! {
            <div class=format!("rc-bubble {class}") style=ring.clone() class:tq-won=move || guess.is_some() && won.get() == guess>
                <p class="tq-tag">{tag.clone()}</p>
                {quote}
                <RichText content=text.clone() options=options.clone() on_link=on_link />
            </div>
        }
    })
}

/// The mark of the player color `color`, as CSS.
fn mark(color: u8) -> String {
    format!("var(--tq-player-{}-mark)", color + 1)
}

/// `thinker`'s dots, `think-Alice`: a live message whose text, `silence`,
/// is empty.
fn dots(thinker: &Thinker, silence: RwSignal<String>) -> Message {
    let id = format!("think-{}", thinker.who);
    Message::streaming(id, thinker.who.clone(), silence)
}
