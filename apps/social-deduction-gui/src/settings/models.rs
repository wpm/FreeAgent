//! The Models tab: a card for each provider, holding its key, the models
//! added from it, and the models it offers that play and are not added
//! yet, most capable first. Models are checked in the background, and a
//! line at the top says while any are.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use social_deduction_stage::settings::{
    AppSettings, Check, FieldError, KeyStatus, Listed, Listing, ModelSettings, Probe, Progress,
    Provider, Trouble, Unchecked, host, is_remote, provider_at,
};

use super::{Cross, Editor, field_id, path};
use crate::tauri;

/// What a provider is, which its card's name explains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A hosted provider the app knows.
    Hosted,
    /// A model server on this computer.
    Local,
    /// A provider the user added.
    Added,
}

impl Kind {
    /// What `provider` is, given the providers `found`, known and local.
    pub fn of(provider: &Provider, found: &[Provider]) -> Kind {
        if Provider::local_port(&provider.base_url).is_some() {
            Kind::Local
        } else if provider_at(found, &provider.base_url).is_some() {
            Kind::Hosted
        } else {
            Kind::Added
        }
    }

    /// What its name's help says.
    pub fn tip(self) -> &'static str {
        match self {
            Kind::Hosted => "A hosted provider. Its address and key variable come filled in.",
            Kind::Local => "A model server found running on this computer.",
            Kind::Added => "A provider you added.",
        }
    }
}

/// `tokens` with its thousands grouped: `1,047,576`.
pub fn grouped(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut out = String::new();
    for (place, digit) in digits.chars().enumerate() {
        if place > 0 && (digits.len() - place).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The number of tokens typed as `text`, its commas and spaces ignored,
/// or none when nothing is typed or it is not a whole number.
pub fn typed_tokens(text: &str) -> Option<u64> {
    let digits: String = text
        .chars()
        .filter(|c| !matches!(c, ',' | ' ' | '_'))
        .collect();
    digits.parse().ok()
}

/// The models of `listing` that play, and that `added`, the ids of the
/// models added, does not hold, in the listing's order.
pub fn available<'a>(listing: &'a Listing, added: &BTreeSet<String>) -> Vec<&'a Listed> {
    listing
        .models
        .iter()
        .filter(|model| !model.hidden && !added.contains(&model.id))
        .filter(|model| listing.checks.get(&model.id).is_some_and(Check::plays))
        .collect()
}

/// Whether the model `id` of `listing` has been checked and does not play.
pub fn stopped(listing: &Listing, id: &str) -> bool {
    listing.checks.get(id).is_some_and(|check| !check.plays())
}

/// How many models of the provider at `base_url` are still to be
/// checked, as `progress` says, if any.
fn remaining(progress: &BTreeMap<String, Progress>, base_url: &str) -> Option<usize> {
    progress
        .get(base_url)
        .filter(|p| p.checked < p.total)
        .map(|p| p.total - p.checked)
}

/// The line at the top of the tab while models are checked, naming the
/// providers of `shown` whose models are, or none when none are:
/// `Checking 2 new models on Local (localhost:1234) in the background.`
pub fn checking_line(progress: &BTreeMap<String, Progress>, shown: &[Provider]) -> Option<String> {
    let checking: Vec<(&str, usize)> = shown
        .iter()
        .filter_map(|p| Some((p.name.as_str(), remaining(progress, &p.base_url)?)))
        .collect();
    let count: usize = checking.iter().map(|(_, count)| count).sum();
    let names: Vec<&str> = checking.iter().map(|(name, _)| *name).collect();
    let names = match names.split_last()? {
        (last, []) => (*last).to_string(),
        (last, rest) => format!("{} and {last}", rest.join(", ")),
    };
    let models = if count == 1 { "model" } else { "models" };
    Some(format!(
        "Checking {count} new {models} on {names} in the background."
    ))
}

/// Why the models of the provider `name` could not be checked, in the
/// user's words, from `unchecked`. An account out of credits is told
/// where to add them, at the host of the first link the provider gave.
pub fn unchecked_line(name: &str, unchecked: &Unchecked) -> String {
    match unchecked.trouble {
        Trouble::NoCredits => {
            let at = unchecked
                .said
                .split_whitespace()
                .find(|word| word.starts_with("https://"))
                .map_or_else(String::new, |link| {
                    format!(" at {}", host(link).trim_end_matches('.'))
                });
            format!("{name}'s account has no credits left. Add credits{at}, then reopen Settings.")
        }
        Trouble::KeyRefused => format!("{name} didn't accept the API key."),
        Trouble::KeyForbidden => format!("{name} didn't let the API key use its models."),
        Trouble::Failed => {
            format!("Some of {name}'s models couldn't be checked. Reopen Settings to try again.")
        }
    }
}

/// What a provider whose `listing` holds no model that plays says: a note
/// giving the reason most of them did not play, the first of the most
/// common in the listing's order, and a tip giving each model's reason.
pub fn not_playing(listing: &Listing) -> (String, String) {
    let reasons: Vec<(&str, &str)> = listing
        .models
        .iter()
        .filter_map(|model| Some((model.id.as_str(), listing.checks.get(&model.id)?.cause()?)))
        .collect();
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (_, why) in &reasons {
        match counts.iter_mut().find(|(each, _)| each == why) {
            Some((_, count)) => *count += 1,
            None => counts.push((why, 1)),
        }
    }
    // The most common, and the first of those in the listing's order.
    let most = counts
        .iter()
        .enumerate()
        .max_by_key(|&(place, &(_, count))| (count, Reverse(place)))
        .map_or("", |(_, &(why, _))| why);
    let note = format!(
        "None of its models plays both games. Most often: {}.",
        most.trim_end_matches('.')
    );
    let tip = reasons
        .iter()
        .map(|(id, why)| format!("{id}: {why}"))
        .collect::<Vec<_>>()
        .join("\n");
    (note, tip)
}

/// Why some models of the provider at `base_url` could not be checked,
/// once checking them has ended, as `progress` says.
fn unchecked<'a>(
    progress: &'a BTreeMap<String, Progress>,
    base_url: &str,
) -> Option<&'a Unchecked> {
    remaining(progress, base_url)
        .is_none()
        .then(|| progress.get(base_url)?.unchecked.as_ref())
        .flatten()
}

/// The line at the top of the tab once no models are being checked: that
/// every model is, unless a provider of `shown` could not check some, as
/// `progress` says.
pub fn checked_line(progress: &BTreeMap<String, Progress>, shown: &[Provider]) -> &'static str {
    if shown
        .iter()
        .any(|provider| unchecked(progress, &provider.base_url).is_some())
    {
        "Some models couldn't be checked. Their providers say why."
    } else {
        "Every model is checked. Only those that play both games are listed."
    }
}

/// The errors among `errors` of the model `name`, any field.
pub fn model_errors(errors: &[FieldError], name: &str) -> Vec<String> {
    errors
        .iter()
        .filter(
            |error| matches!(&error.path[..], [models, of, ..] if models == "models" && of == name),
        )
        .map(|error| error.message.clone())
        .collect()
}

/// Change the settings with `change`, and send them to be stored, unless
/// it fails.
fn attempt(
    editor: Editor,
    change: impl FnOnce(&mut AppSettings) -> Result<(), String>,
) -> Result<(), String> {
    let mut changed = editor.settings.get_untracked();
    change(&mut changed)?;
    editor.settings.set(changed);
    editor.send();
    Ok(())
}

/// What the Models tab knows of the providers.
#[derive(Clone, Copy)]
struct Catalog {
    /// The providers found, known and local.
    found: RwSignal<Vec<Provider>>,
    /// Each provider's listing, by base URL, once it has come.
    listings: RwSignal<BTreeMap<String, Option<Listing>>>,
    /// How far probing each provider has got, by base URL.
    progress: RwSignal<BTreeMap<String, Progress>>,
    /// Where each key variable's key is.
    keys: RwSignal<BTreeMap<String, KeyStatus>>,
}

impl Catalog {
    /// Fetch the listing of the provider at `base_url`, its key in the
    /// variable `api_key_env` names, its probes forgotten first when
    /// `forget` says.
    fn fetch(self, base_url: String, api_key_env: Option<String>, forget: bool) {
        self.listings.update(|listings| {
            listings.entry(base_url.clone()).or_default();
        });
        spawn_local(async move {
            let listing = tauri::listing(base_url.clone(), api_key_env, forget)
                .await
                .unwrap_or_else(|failure| Listing {
                    error: Some(failure),
                    ..Listing::default()
                });
            self.listings.update(|listings| {
                listings.insert(base_url, Some(listing));
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
            let Some(listing) = listings.get_mut(&probe.base_url).and_then(Option::as_mut) else {
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

    /// The listing of the provider at `base_url`, as it changes, once it
    /// has come.
    fn fetched(self, base_url: String) -> Memo<Option<Listing>> {
        Memo::new(move |_| {
            self.listings
                .with(|listings| listings.get(&base_url).cloned().flatten())
        })
    }
}

/// The Models tab: the line saying what is being checked, a card for
/// each provider, and **Add provider…**.
#[component]
pub(super) fn Models(editor: Editor) -> impl IntoView {
    let catalog = Catalog {
        found: RwSignal::new(Vec::new()),
        listings: RwSignal::new(BTreeMap::new()),
        progress: RwSignal::new(BTreeMap::new()),
        keys: RwSignal::new(BTreeMap::new()),
    };
    spawn_local(async move {
        if let Ok(found) = tauri::providers().await {
            catalog.found.set(found);
        }
    });
    spawn_local(async move {
        if let Ok(all) = tauri::checking().await {
            catalog.progress.update(|progress| {
                for each in all {
                    progress.entry(each.base_url.clone()).or_insert(each);
                }
            });
        }
    });
    tauri::listen("probe", move |probe: Probe| catalog.probed(probe));
    tauri::listen("progress", move |progress: Progress| {
        catalog.progress.update(|all| {
            all.insert(progress.base_url.clone(), progress);
        });
    });
    let shown = Memo::new(move |_| {
        catalog
            .found
            .with(|found| editor.settings.with(|s| s.providers_shown(found)))
    });
    // Each provider shown is listed once each time the tab opens.
    Effect::new(move |_| {
        for provider in shown.get() {
            let listed = catalog
                .listings
                .with_untracked(|listings| listings.contains_key(&provider.base_url));
            if !listed {
                catalog.fetch(provider.base_url, provider.api_key_env, false);
            }
        }
    });
    let status = move || {
        let line = catalog
            .progress
            .with(|progress| shown.with(|shown| checking_line(progress, shown)));
        match line {
            Some(line) => view! {
                <div class="sd-status" data-tip="Checks run in the background and never block you. Local models are checked one at a time, and not while a game is playing.">
                    <span class="sd-spinner"></span>
                    <span>{line}</span>
                </div>
            }
            .into_any(),
            None => {
                let checked = catalog
                    .progress
                    .with(|progress| shown.with(|shown| checked_line(progress, shown)));
                view! {
                    <div class="sd-status sd-status-idle" data-tip="Models are checked on first launch and after each update, and again when a provider lists a new model, a key changes, or Settings opens with some not yet checked.">
                        <span>{checked}</span>
                    </div>
                }
                .into_any()
            }
        }
    };
    view! {
        <div class="sd-tab-head">
            <h2>"Models"</h2>
        </div>
        {status}
        <For
            each=move || shown.get()
            key=|provider| (provider.name.clone(), provider.base_url.clone(), provider.api_key_env.clone())
            let:provider
        >
            <ProviderCard editor=editor catalog=catalog provider=provider />
        </For>
        <AddProvider editor=editor />
    }
}

/// A provider's card: its name and base URL, its key, the models added
/// from it, and those it offers that play and are not added.
#[component]
fn ProviderCard(editor: Editor, catalog: Catalog, provider: Provider) -> impl IntoView {
    let kind = catalog
        .found
        .with_untracked(|found| Kind::of(&provider, found));
    let Provider {
        name,
        base_url,
        api_key_env,
    } = provider.clone();
    let fetched = catalog.fetched(base_url.clone());
    let key = match api_key_env.clone() {
        Some(variable) => {
            let (base_url, api_key_env) = (base_url.clone(), api_key_env.clone());
            let changed = Callback::new(move |()| {
                catalog.fetch(base_url.clone(), api_key_env.clone(), true);
            });
            view! { <KeyField catalog=catalog variable=variable changed=changed /> }.into_any()
        }
        None => view! {
            <div class="sd-key">
                <span class="sd-chip" data-tip="A model server on this computer. It doesn't need an API key.">"No key needed"</span>
            </div>
        }
        .into_any(),
    };
    let remove = {
        let base_url = base_url.clone();
        move |_| {
            let base_url = base_url.clone();
            editor.edit(move |s| s.remove_provider(&base_url));
        }
    };
    let names = {
        let mine = [provider.clone()];
        Memo::new(move |_| {
            editor.settings.with(|s| {
                s.models
                    .iter()
                    .filter(|(_, model)| provider_at(&mine, &model.base_url).is_some())
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>()
            })
        })
    };
    view! {
        <section class="sd-card sd-provider" aria-label=name.clone()>
            <div class="sd-card-head">
                <h4 data-tip=kind.tip()>{name.clone()}</h4>
                <code class="sd-url" data-tip="The address this provider's models are reached at.">{base_url.clone()}</code>
                {(kind == Kind::Added).then(|| view! {
                    <button class="sd-link" on:click=remove data-tip="Remove this provider and its models.">"Remove provider"</button>
                })}
            </div>
            {key}
            <Show when=move || names.with(|names| !names.is_empty())>
                <h5 class="sd-label" data-tip="The models from this provider your strategies can use.">"Your models"</h5>
                <div class="sd-models">
                    <For each=move || names.get() key=Clone::clone let:name>
                        <ModelRow editor=editor fetched=fetched name=name />
                    </For>
                </div>
            </Show>
            <Available editor=editor catalog=catalog provider=provider.clone() fetched=fetched names=names />
        </section>
    }
}

/// The models a provider offers that play and are not added, each with
/// **Add**, which asks for its name; or why there are none; and why some
/// could not be checked, once checking ends.
#[component]
fn Available(
    editor: Editor,
    catalog: Catalog,
    provider: Provider,
    fetched: Memo<Option<Listing>>,
    names: Memo<Vec<String>>,
) -> impl IntoView {
    let local = !is_remote(&provider.base_url);
    let added = Memo::new(move |_| {
        names.with(|names| {
            editor.settings.with(|s| {
                names
                    .iter()
                    .filter_map(|name| s.models.get(name).map(|m| m.id.clone()))
                    .collect::<BTreeSet<_>>()
            })
        })
    });
    let adding = RwSignal::new(None::<String>);
    let pending = {
        let base_url = provider.base_url.clone();
        Memo::new(move |_| {
            catalog
                .progress
                .with(|progress| remaining(progress, &base_url))
        })
    };
    // Why some of its models could not be checked, once checking ends, for
    // a provider that lists them.
    let trouble = {
        let base_url = provider.base_url.clone();
        Memo::new(move |_| {
            let listed =
                fetched.with(|listing| listing.as_ref().is_some_and(|l| l.error.is_none()));
            let unchecked = catalog
                .progress
                .with(|progress| unchecked(progress, &base_url).cloned());
            unchecked.filter(|_| listed)
        })
    };
    let rows = Memo::new(move |_| {
        fetched.with(|listing| {
            listing.as_ref().map_or_else(Vec::new, |listing| {
                added.with(|added| available(listing, added).into_iter().cloned().collect())
            })
        })
    });
    let still = move || {
        let listed = fetched.with(|listing| listing.as_ref().is_some_and(|l| l.error.is_none()));
        let count = pending.get().filter(|_| listed)?;
        Some(view! {
            <p class="sd-note" data-tip="Checked in the background. Each appears here if it plays.">{format!("Checking {count} more…")}</p>
        })
    };
    // Why none are available, when none are.
    let none = move || {
        let note = |text: &'static str| view! { <p class="sd-note">{text}</p> }.into_any();
        fetched.with(|listing| {
            let Some(listing) = listing else {
                return Some(note("Listing its models…"));
            };
            if let Some(error) = &listing.error {
                return Some(view! { <p class="sd-field-error">{error.clone()}</p> }.into_any());
            }
            if listing.models.iter().all(|model| model.hidden) {
                return Some(if local {
                    view! {
                        <p class="sd-note" data-tip="The server answered at this address but has no model loaded. Load one and it will appear here once it has been checked.">
                            "The server is running, but no model is loaded."
                        </p>
                    }
                    .into_any()
                } else {
                    note("It lists no models.")
                });
            }
            if !rows.with(Vec::is_empty)
                || pending.with(Option::is_some)
                || trouble.with(Option::is_some)
            {
                return None;
            }
            let checked = listing
                .models
                .iter()
                .all(|model| model.hidden || listing.checks.contains_key(&model.id));
            if !checked {
                return Some(note("Its models haven't been checked yet."));
            }
            if !added.with(BTreeSet::is_empty) {
                return Some(note(
                    "Every model this provider has that plays is already added.",
                ));
            }
            let (text, tip) = not_playing(listing);
            Some(view! { <p class="sd-note" data-tip=tip>{text}</p> }.into_any())
        })
    };
    let troubled = {
        let name = provider.name.clone();
        move || {
            let unchecked = trouble.get()?;
            Some(view! {
                <p class="sd-field-error" data-tip=unchecked.said.clone()>
                    {unchecked_line(&name, &unchecked)}
                </p>
            })
        }
    };
    let provider = StoredValue::new(provider);
    view! {
        {troubled}
        {none}
        <Show when=move || rows.with(|rows| !rows.is_empty())>
            <h5 class="sd-label" data-tip="Models from this provider that play both games and aren't added yet, most capable first.">"Available"</h5>
            <div class="sd-models">
                <For each=move || rows.get() key=|model| model.id.clone() let:model>
                    <Offered editor=editor provider=provider.get_value() model=model adding=adding />
                </For>
            </div>
        </Show>
        {still}
    }
}

/// A model a provider offers: its id, its context window and **Add**, or,
/// once **Add** is pressed, a field for its name.
#[component]
fn Offered(
    editor: Editor,
    provider: Provider,
    model: Listed,
    adding: RwSignal<Option<String>>,
) -> impl IntoView {
    let Listed {
        id, context_window, ..
    } = model;
    let field = NodeRef::<html::Input>::new();
    let problem = RwSignal::new(None::<String>);
    let confirm = {
        let id = id.clone();
        move || {
            let Some(input) = field.get_untracked() else {
                return;
            };
            let model = ModelSettings {
                base_url: provider.base_url.clone(),
                id: id.clone(),
                api_key_env: provider.api_key_env.clone(),
                ..ModelSettings::default()
            };
            match attempt(editor, |s| s.add_model(&input.value(), model)) {
                Ok(()) => adding.set(None),
                Err(why) => problem.set(Some(why)),
            }
        }
    };
    let opened = {
        let id = id.clone();
        move || adding.get().as_deref() == Some(id.as_str())
    };
    let open = {
        let id = id.clone();
        move |_| {
            problem.set(None);
            adding.set(Some(id.clone()));
        }
    };
    // The name field is focused, its text chosen, as it opens.
    Effect::new(move |_| {
        if let Some(input) = field.get() {
            let _ = input.focus();
            input.select();
        }
    });
    let typed = confirm.clone();
    let shown_id = id.clone();
    view! {
        <Show
            when=opened
            fallback=move || view! {
                <div class="sd-model sd-offered">
                    <code class="sd-id">{id.clone()}</code>
                    <span class="sd-note" data-tip="The context window the provider reports.">{format!("{} tokens", grouped(context_window))}</span>
                    <button class="sd-small" on:click=open.clone() data-tip="Make this model available to strategies. You name it next.">"Add"</button>
                </div>
            }
        >
            <div class="sd-model sd-adding">
                <code class="sd-id">{shown_id.clone()}</code>
                <input
                    type="text"
                    node_ref=field
                    spellcheck="false"
                    autocomplete="off"
                    aria-label="Name for the new model"
                    data-tip="Strategies pick the model by this name. It starts as the model id; shorten it if you like."
                    prop:value=move || adding.get().unwrap_or_default()
                    on:keydown={
                        let typed = typed.clone();
                        move |event| match event.key().as_str() {
                            "Enter" => typed(),
                            "Escape" => adding.set(None),
                            _ => {}
                        }
                    }
                />
                <button class="sd-primary sd-small" on:click={
                    let confirm = confirm.clone();
                    move |_| confirm()
                }>"Add"</button>
                <button class="sd-small" on:click=move |_| adding.set(None)>"Cancel"</button>
            </div>
            {move || problem.get().map(|why| view! { <p class="sd-field-error">{why}</p> })}
        </Show>
    }
}

/// A model added: its name, which is edited in place, its id, its context
/// window, **×**, and, when it no longer plays, a warning naming the
/// strategies that ask it.
#[component]
fn ModelRow(editor: Editor, fetched: Memo<Option<Listing>>, name: String) -> impl IntoView {
    let model = {
        let name = name.clone();
        Memo::new(move |_| {
            editor
                .settings
                .with(|s| s.models.get(&name).cloned().unwrap_or_default())
        })
    };
    let users = {
        let name = name.clone();
        Memo::new(move |_| editor.settings.with(|s| s.users_of_model(&name)))
    };
    let errors = {
        let name = name.clone();
        Memo::new(move |_| editor.errors.with(|errors| model_errors(errors, &name)))
    };
    let listed = Memo::new(move |_| {
        model.with(|m| {
            fetched.with(|listing| {
                let listing = listing.as_ref()?;
                let window = listing
                    .models
                    .iter()
                    .find(|listed| listed.id == m.id)
                    .map(|listed| listed.context_window);
                Some((window, stopped(listing, &m.id)))
            })
        })
    });
    let figure = move || listed.get().and_then(|(window, _)| window);
    let problem = RwSignal::new(None::<String>);
    let renamed = {
        let name = name.clone();
        move |event| {
            let input = event_target::<web_sys::HtmlInputElement>(&event);
            let wanted = input.value();
            match attempt(editor, |s| s.rename_model(&name, &wanted)) {
                Ok(()) => problem.set(None),
                Err(why) => {
                    input.set_value(&name);
                    problem.set(Some(why));
                }
            }
        }
    };
    let name_tip = move || {
        let users = users.get();
        if users.is_empty() {
            "The name strategies pick this model by. No strategy uses it yet.".to_string()
        } else {
            format!(
                "The name strategies pick this model by. Used by {}. Renaming updates them.",
                users.join(", ")
            )
        }
    };
    let set_window = {
        let name = name.clone();
        move |window: Option<u64>| {
            editor.edit_model(&name, |m| m.context_window = window);
        }
    };
    let typed = {
        let set_window = set_window.clone();
        move |event| set_window(typed_tokens(&event_target_value(&event)))
    };
    let window_tip = move || {
        match figure() {
        Some(tokens) => format!(
            "How many tokens of the game so far the model is sent. Empty means the provider's figure, {}. A smaller number keeps prompts short and cheap.",
            grouped(tokens)
        ),
        None => "How many tokens of the game so far the model is sent. Empty means the provider's figure. A smaller number keeps prompts short and cheap.".to_string(),
    }
    };
    let removed = name.clone();
    let remove_tip = move || {
        if users.with(Vec::is_empty) {
            "Remove this model."
        } else {
            "Remove this model. Strategies that use it will need another."
        }
    };
    view! {
        <div class="sd-model" class:sd-warned=move || listed.get().is_some_and(|(_, stopped)| stopped)>
            <input
                type="text"
                class="sd-name"
                id=field_id(&path(&["models", &name]))
                spellcheck="false"
                autocomplete="off"
                aria-label="Model name"
                data-tip=name_tip
                value=name.clone()
                on:change=renamed
                on:keydown=|event| {
                    if event.key() == "Enter" {
                        let _ = event_target::<web_sys::HtmlElement>(&event).blur();
                    }
                }
            />
            <code class="sd-id" id=field_id(&path(&["models", &name, "id"])) data-tip="The id the provider lists this model under.">
                {move || model.with(|m| m.id.clone())}
            </code>
            <span class="sd-window">
                <input
                    type="text"
                    inputmode="numeric"
                    id=field_id(&path(&["models", &name, "context_window"]))
                    aria-label="Context window in tokens"
                    data-tip=window_tip
                    placeholder=move || figure().map_or_else(|| "the provider's".to_string(), grouped)
                    prop:value=move || model.with(|m| m.context_window.map(grouped).unwrap_or_default())
                    on:change=typed
                />
                <span class="sd-note">"tokens"</span>
                <Show when=move || model.with(|m| m.context_window.is_some())>
                    <button class="sd-link" data-tip="Go back to the provider's figure." on:click={
                        let set_window = set_window.clone();
                        move |_| set_window(None)
                    }>"Reset"</button>
                </Show>
            </span>
            <button
                class="sd-remove"
                aria-label=format!("Remove {name}")
                data-tip=remove_tip
                on:click=move |_| {
                    let removed = removed.clone();
                    editor.edit(move |s| {
                        s.models.remove(&removed);
                    });
                }
            >
                <Cross />
            </button>
            {move || listed.get().is_some_and(|(_, stopped)| stopped).then(|| {
                let users = users.get();
                let used = if users.is_empty() { String::new() } else { format!(" Used by {}.", users.join(", ")) };
                view! {
                    <p class="sd-warning" data-tip="It failed the check after the last app update, or the provider changed the model behind its id. Games using it will likely fail. Pick another model for its strategies, or remove it.">
                        {format!("⚠ No longer plays.{used}")}
                    </p>
                }
            })}
            {move || problem.get().map(|why| view! { <p class="sd-field-error">{why}</p> })}
            {move || errors.get().into_iter().map(|error| view! { <p class="sd-field-error">{error}</p> }).collect_view()}
        </div>
    }
}

/// A provider's key: where it is, by its variable's name, and how to
/// store it in the keychain or remove it from there. A key is read from
/// its field when **Save to keychain** is pressed, and the field is
/// cleared as soon as the keychain answers; `changed` hears that the key
/// changed.
#[component]
fn KeyField(catalog: Catalog, variable: String, changed: Callback<()>) -> impl IntoView {
    catalog.key_status(variable.clone());
    let status = {
        let variable = variable.clone();
        Memo::new(move |_| catalog.keys.with(|keys| keys.get(&variable).cloned()))
    };
    let field = NodeRef::<html::Input>::new();
    let failure = RwSignal::new(None::<String>);
    let opened = RwSignal::new(false);
    // What the keychain answered: the failure, if any; otherwise the key
    // changed.
    let answered = {
        let variable = variable.clone();
        move |result: Result<(), String>| {
            let ok = result.is_ok();
            failure.set(result.err());
            catalog.key_status(variable.clone());
            if ok {
                opened.set(false);
                changed.run(());
            }
        }
    };
    let save = {
        let (variable, answered) = (variable.clone(), answered.clone());
        move |_| {
            let Some(input) = field.get_untracked() else {
                return;
            };
            let key = input.value();
            if key.trim().is_empty() {
                return;
            }
            let (variable, answered) = (variable.clone(), answered.clone());
            spawn_local(async move {
                let result = tauri::key_store(variable, key).await;
                input.set_value("");
                answered(result);
            });
        }
    };
    let remove = {
        let variable = variable.clone();
        move |_| {
            let (variable, answered) = (variable.clone(), answered.clone());
            spawn_local(async move { answered(tauri::key_remove(variable).await) });
        }
    };
    let chip = {
        let variable = variable.clone();
        move || {
            let status = status.get()?;
            let (tip, good) = match &status {
                KeyStatus::InEnvironment => (
                    "Read from the environment the app started in. A key stored in the keychain would take priority.",
                    true,
                ),
                KeyStatus::InKeychain => (
                    "Stored, encrypted, in your system keychain. It takes priority over the environment variable, and the app never shows or logs it.",
                    true,
                ),
                KeyStatus::Missing => (
                    "No key in the keychain or the environment, so this provider's models can't be listed or checked.",
                    false,
                ),
                KeyStatus::Unavailable(_) => ("The system keychain couldn't be asked.", false),
            };
            Some(view! {
                <span class="sd-chip" class:sd-chip-good=good class:sd-chip-bad=!good data-tip=tip>
                    <code>{variable.clone()}</code>
                    {format!(" {}", status.told())}
                </span>
            })
        }
    };
    let form_shown = move || opened.get() || status.get().is_some_and(|status| !status.is_set());
    view! {
        <div class="sd-key">
            {chip}
            <Show when=move || status.get() == Some(KeyStatus::InKeychain)>
                <button class="sd-link" on:click=remove.clone() data-tip="Delete the key from the keychain. The environment variable is used again, if it is set.">"Remove from keychain"</button>
            </Show>
            <Show when=move || status.get() == Some(KeyStatus::InEnvironment) && !opened.get()>
                <button class="sd-link" on:click=move |_| opened.set(true) data-tip="Store a key in the system keychain instead. It takes priority over the environment variable.">"Store a key in the keychain…"</button>
            </Show>
            <Show when=form_shown>
                <input type="password" node_ref=field autocomplete="off" placeholder="Paste an API key" aria-label="API key" data-tip="Saved to the system keychain, never to a file, and never shown again." />
                <button class="sd-primary" on:click=save.clone() data-tip="Store this key in the keychain. The models are listed and checked again with it.">"Save to keychain"</button>
            </Show>
        </div>
        {move || failure.get().map(|failure| view! { <p class="sd-field-error">{failure}</p> })}
    }
}

/// **Add provider…**, which opens fields for another OpenAI-compatible
/// provider's name, base URL and key variable.
#[component]
fn AddProvider(editor: Editor) -> impl IntoView {
    let opened = RwSignal::new(false);
    let problem = RwSignal::new(None::<String>);
    let (name, base_url, variable) = (
        NodeRef::<html::Input>::new(),
        NodeRef::<html::Input>::new(),
        NodeRef::<html::Input>::new(),
    );
    let value = |field: NodeRef<html::Input>| {
        field
            .get_untracked()
            .map(|input| input.value())
            .unwrap_or_default()
    };
    let confirm = move |_| {
        let variable = value(variable).trim().to_string();
        let provider = Provider {
            name: value(name),
            base_url: value(base_url),
            api_key_env: (!variable.is_empty()).then_some(variable),
        };
        match attempt(editor, |s| s.add_provider(provider)) {
            Ok(()) => {
                problem.set(None);
                opened.set(false);
            }
            Err(why) => problem.set(Some(why)),
        }
    };
    view! {
        <Show
            when=move || opened.get()
            fallback=move || view! {
                <div class="sd-row">
                    <button on:click=move |_| opened.set(true) data-tip="Add another OpenAI-compatible provider by its address.">"Add provider…"</button>
                </div>
            }
        >
            <div class="sd-card sd-add-provider">
                <label class="sd-field">
                    <span>"Name"</span>
                    <input type="text" node_ref=name spellcheck="false" placeholder="Provider name" data-tip="What this provider is called in Settings." />
                </label>
                <label class="sd-field">
                    <span>"Base URL"</span>
                    <input type="text" node_ref=base_url spellcheck="false" placeholder="https://api.example.com/v1" data-tip="The root of an OpenAI-compatible API, ending in /v1." />
                </label>
                <label class="sd-field">
                    <span>"Key variable"</span>
                    <input type="text" node_ref=variable spellcheck="false" placeholder="EXAMPLE_API_KEY (optional)" data-tip="The environment variable the key is read from, and the name it is stored under in the keychain. Leave it empty if the server needs no key." />
                </label>
                <div class="sd-row">
                    <button class="sd-primary" on:click=confirm>"Add provider"</button>
                    <button on:click=move |_| {
                        problem.set(None);
                        opened.set(false);
                    }>"Cancel"</button>
                </div>
                {move || problem.get().map(|why| view! { <p class="sd-field-error">{why}</p> })}
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider called `name` at `base_url`.
    fn provider(name: &str, base_url: &str) -> Provider {
        Provider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: None,
        }
    }

    #[test]
    fn a_provider_is_hosted_local_or_added() {
        let openai = provider("OpenAI", "https://api.openai.com/v1");
        let found = [openai.clone(), Provider::local(1234)];
        assert_eq!(Kind::of(&openai, &found), Kind::Hosted);
        assert_eq!(Kind::of(&Provider::local(9999), &found), Kind::Local);
        let example = provider("Example", "https://api.example.com/v1");
        assert_eq!(Kind::of(&example, &found), Kind::Added);
        assert_eq!(Kind::Added.tip(), "A provider you added.");
    }

    #[test]
    fn tokens_are_grouped_by_thousands_and_read_back_without_the_commas() {
        assert_eq!(grouped(1_047_576), "1,047,576");
        assert_eq!(grouped(200_000), "200,000");
        assert_eq!(grouped(4096), "4,096");
        assert_eq!(grouped(512), "512");
        assert_eq!(typed_tokens("1,047,576"), Some(1_047_576));
        assert_eq!(typed_tokens(" 32 768 "), Some(32768));
        assert_eq!(typed_tokens(""), None);
        assert_eq!(typed_tokens("lots"), None);
    }

    /// A listing of `ids`, of which those starting `text-embedding` are
    /// hidden, with `checks`.
    fn listing(ids: &[&str], checks: &[(&str, Check)]) -> Listing {
        Listing {
            models: ids
                .iter()
                .map(|id| Listed {
                    id: id.to_string(),
                    context_window: 200_000,
                    hidden: id.starts_with("text-embedding"),
                })
                .collect(),
            error: None,
            checks: checks
                .iter()
                .map(|(id, check)| (id.to_string(), check.clone()))
                .collect(),
        }
    }

    #[test]
    fn only_models_that_play_and_are_not_added_are_available() {
        let no = Check::DidNotPlay("no tools".to_string());
        let listing = listing(
            &["opus", "sonnet", "haiku", "mute", "new", "text-embedding-3"],
            &[
                ("opus", Check::Plays),
                ("sonnet", Check::PlaysUnforced),
                ("haiku", Check::Plays),
                ("mute", no),
                ("text-embedding-3", Check::Plays),
            ],
        );
        let added = BTreeSet::from(["haiku".to_string()]);
        let ids: Vec<&str> = available(&listing, &added)
            .into_iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(ids, ["opus", "sonnet"]);
        assert!(stopped(&listing, "mute"));
        assert!(!stopped(&listing, "opus") && !stopped(&listing, "new"));
    }

    #[test]
    fn the_status_line_counts_the_models_still_to_check_and_names_their_providers() {
        let progress = |base_url: &str, checked, total| Progress {
            base_url: base_url.to_string(),
            checked,
            total,
            unchecked: None,
        };
        let local = "http://localhost:1234/v1";
        let openai = "https://api.openai.com/v1";
        let shown = [Provider::local(1234), provider("OpenAI", openai)];
        let all = |each: &[Progress]| {
            each.iter()
                .map(|p| (p.base_url.clone(), p.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(checking_line(&all(&[progress(openai, 3, 3)]), &shown), None);
        assert_eq!(
            checking_line(&all(&[progress(local, 1, 3)]), &shown).as_deref(),
            Some("Checking 2 new models on Local (localhost:1234) in the background.")
        );
        assert_eq!(
            checking_line(
                &all(&[
                    progress(local, 2, 3),
                    progress(openai, 0, 2),
                    progress("http://x/v1", 0, 1)
                ]),
                &shown
            )
            .as_deref(),
            Some("Checking 3 new models on Local (localhost:1234) and OpenAI in the background.")
        );
        let three = [
            shown[0].clone(),
            shown[1].clone(),
            provider("X", "http://x/v1"),
        ];
        assert_eq!(
            checking_line(
                &all(&[
                    progress(local, 2, 3),
                    progress(openai, 0, 2),
                    progress("http://x/v1", 0, 1)
                ]),
                &three
            )
            .as_deref(),
            Some(
                "Checking 4 new models on Local (localhost:1234), OpenAI and X in the background."
            )
        );
    }

    #[test]
    fn once_checking_ends_the_status_line_says_whether_any_model_could_not_be_checked() {
        let openai = "https://api.openai.com/v1";
        let shown = [provider("OpenAI", openai)];
        let ended = |unchecked: Option<Unchecked>| {
            BTreeMap::from([(
                openai.to_string(),
                Progress {
                    base_url: openai.to_string(),
                    checked: 2,
                    total: 2,
                    unchecked,
                },
            )])
        };
        let every = "Every model is checked. Only those that play both games are listed.";
        assert_eq!(checked_line(&BTreeMap::new(), &shown), every);
        assert_eq!(checked_line(&ended(None), &shown), every);
        let broke = ended(Some(unchecked(Trouble::NoCredits, "")));
        assert_eq!(
            checked_line(&broke, &shown),
            "Some models couldn't be checked. Their providers say why."
        );
        // A provider not shown is not counted.
        assert_eq!(checked_line(&broke, &[]), every);
    }

    /// Why a model could not be checked: `trouble`, the provider having
    /// said `said`.
    fn unchecked(trouble: Trouble, said: &str) -> Unchecked {
        Unchecked {
            trouble,
            said: said.to_string(),
        }
    }

    #[test]
    fn a_provider_whose_models_could_not_be_checked_says_why_in_the_user_s_words() {
        let said = "You have no credits remaining. Add credits to continue using the API at https://platform.openai.com/settings/organization/billing/.";
        assert_eq!(
            unchecked_line("OpenAI", &unchecked(Trouble::NoCredits, said)),
            "OpenAI's account has no credits left. Add credits at platform.openai.com, then reopen Settings."
        );
        assert_eq!(
            unchecked_line(
                "Example",
                &unchecked(Trouble::NoCredits, "Pay at https://pay.example.")
            ),
            "Example's account has no credits left. Add credits at pay.example, then reopen Settings."
        );
        assert_eq!(
            unchecked_line("Example", &unchecked(Trouble::NoCredits, "Pay up.")),
            "Example's account has no credits left. Add credits, then reopen Settings."
        );
        assert_eq!(
            unchecked_line("OpenAI", &unchecked(Trouble::KeyRefused, "")),
            "OpenAI didn't accept the API key."
        );
        assert_eq!(
            unchecked_line("OpenAI", &unchecked(Trouble::KeyForbidden, "")),
            "OpenAI didn't let the API key use its models."
        );
        assert_eq!(
            unchecked_line("Local (localhost:1234)", &unchecked(Trouble::Failed, "")),
            "Some of Local (localhost:1234)'s models couldn't be checked. Reopen Settings to try again."
        );
    }

    #[test]
    fn models_that_do_not_play_are_told_by_their_most_common_reason_and_each_in_the_tip() {
        let no = |cause: &str| {
            Check::DidNotPlay(format!(
                "m did not play Werewolf: did not answer a vote: {cause}"
            ))
        };
        let listing = listing(
            &["a", "b", "c", "d", "text-embedding-3"],
            &[
                ("a", no("it made no tool call")),
                ("b", no("it chose no one")),
                ("c", no("it chose no one")),
                ("d", no("it made no tool call")),
            ],
        );
        let (note, tip) = not_playing(&listing);
        assert_eq!(
            note,
            "None of its models plays both games. Most often: it made no tool call."
        );
        assert_eq!(
            tip,
            "a: it made no tool call\nb: it chose no one\nc: it chose no one\nd: it made no tool call"
        );
    }

    #[test]
    fn a_model_s_errors_are_those_at_any_of_its_fields() {
        let errors = [
            FieldError::new(&["models", "gpt", "id"], "needs an id"),
            FieldError::new(&["models", "gpt", "context_window"], "too small"),
            FieldError::new(&["models", "gpt-2", "id"], "other"),
            FieldError::new(&["werewolf", "gpt"], "not a model"),
        ];
        assert_eq!(model_errors(&errors, "gpt"), ["needs an id", "too small"]);
    }
}
