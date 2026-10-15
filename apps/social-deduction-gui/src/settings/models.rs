//! The Models tab: the providers, with their keys, and the shared models.

use std::collections::{BTreeMap, BTreeSet};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use social_deduction_stage::settings::{
    AppSettings, Check, KeyStatus, Listing, ModelSettings, Probe, Progress, Provider, is_remote,
    unique,
};

use super::{Choice, Editor, Problem, TextField, field_id, path};
use crate::tauri;

/// The provider of `providers` at `base_url`, if there is one.
pub fn provider_at<'a>(providers: &'a [Provider], base_url: &str) -> Option<&'a Provider> {
    let trimmed = base_url.trim_end_matches('/');
    providers
        .iter()
        .find(|provider| provider.base_url.trim_end_matches('/') == trimmed)
}

/// What the model id dropdown offers of `listing`: each model that
/// plays, with its mark, or, with `all`, every model, each with its mark
/// or none.
pub fn offered(listing: &Listing, all: bool) -> Vec<(String, String)> {
    listing
        .models
        .iter()
        .filter_map(|model| {
            let check = listing.checks.get(&model.id);
            let plays = check.is_some_and(Check::plays);
            if !all && !plays {
                return None;
            }
            let mark = match (check, model.hidden) {
                (Some(check), _) => check.mark(),
                (None, true) => "—",
                (None, false) => "?",
            };
            Some((model.id.clone(), mark.to_string()))
        })
        .collect()
}

/// A provider's listing as the window has it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Fetched {
    /// Whether it is being fetched.
    loading: bool,
    /// The listing, once it has come.
    listing: Option<Listing>,
}

/// What the Models tab knows of the providers.
#[derive(Clone, Copy)]
struct Catalog {
    /// The providers, known and local.
    providers: RwSignal<Vec<Provider>>,
    /// Each provider's listing, by base URL.
    listings: RwSignal<BTreeMap<String, Fetched>>,
    /// How far probing each provider has got, by base URL.
    progress: RwSignal<BTreeMap<String, Progress>>,
    /// Where each key variable's key is.
    keys: RwSignal<BTreeMap<String, KeyStatus>>,
}

impl Catalog {
    /// Fetch the listing of the provider at `base_url`, its key in the
    /// variable `api_key_env` names, afresh when `refresh` says.
    fn fetch(self, base_url: String, api_key_env: Option<String>, refresh: bool) {
        if base_url.trim().is_empty() {
            return;
        }
        self.listings.update(|listings| {
            let fetched = listings.entry(base_url.clone()).or_default();
            fetched.loading = true;
            if refresh {
                fetched.listing = None;
            }
        });
        spawn_local(async move {
            let listing = tauri::listing(base_url.clone(), api_key_env, refresh)
                .await
                .unwrap_or_else(|failure| Listing {
                    error: Some(failure),
                    ..Listing::default()
                });
            self.listings.update(|listings| {
                listings.insert(
                    base_url,
                    Fetched {
                        loading: false,
                        listing: Some(listing),
                    },
                );
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
            let Some(listing) = listings
                .get_mut(&probe.base_url)
                .and_then(|fetched| fetched.listing.as_mut())
            else {
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
}

/// The providers the Models tab shows: those known or found, and each
/// base URL a model names that is neither, as a local server when it is
/// one on this machine, whether or not it is up, and otherwise as
/// **Other**.
fn shown_providers(providers: &[Provider], settings: &AppSettings) -> Vec<Provider> {
    let mut shown = providers.to_vec();
    for model in settings.models.values() {
        if model.base_url.trim().is_empty() || provider_at(&shown, &model.base_url).is_some() {
            continue;
        }
        shown.push(match Provider::local_port(&model.base_url) {
            Some(port) => Provider::local(port),
            None => Provider {
                name: format!("Other ({})", model.base_url),
                base_url: model.base_url.clone(),
                api_key_env: model.api_key_env.clone(),
            },
        });
    }
    shown
}

/// Whether `base_url` is whole, as far as can be told while it is typed:
/// an `http` or `https` URL ending in `/v1`.
pub fn whole(base_url: &str) -> bool {
    let base_url = base_url.trim().trim_end_matches('/');
    (base_url.starts_with("http://") || base_url.starts_with("https://"))
        && base_url.ends_with("/v1")
}

/// The Models tab: the providers, with their keys, and a card for each
/// model, with **Add Model**.
#[component]
pub(super) fn Models(editor: Editor) -> impl IntoView {
    let catalog = Catalog {
        providers: RwSignal::new(Vec::new()),
        listings: RwSignal::new(BTreeMap::new()),
        progress: RwSignal::new(BTreeMap::new()),
        keys: RwSignal::new(BTreeMap::new()),
    };
    spawn_local(async move {
        if let Ok(found) = tauri::providers().await {
            catalog.providers.set(found);
        }
    });
    tauri::listen("probe", move |probe: Probe| catalog.probed(probe));
    tauri::listen("progress", move |progress: Progress| {
        catalog.progress.update(|all| {
            all.insert(progress.base_url.clone(), progress);
        });
    });
    // Each provider a model uses is listed once its base URL is whole.
    let used = Memo::new(move |_| {
        editor.settings.with(|s| {
            s.models
                .values()
                .filter(|model| whole(&model.base_url))
                .map(|model| (model.base_url.clone(), model.api_key_env.clone()))
                .collect::<BTreeSet<_>>()
        })
    });
    Effect::new(move |_| {
        for (base_url, api_key_env) in used.get() {
            let fetched = catalog
                .listings
                .with_untracked(|listings| listings.contains_key(&base_url));
            if !fetched {
                catalog.fetch(base_url, api_key_env, false);
            }
        }
    });
    let shown = Memo::new(move |_| {
        catalog
            .providers
            .with(|providers| editor.settings.with(|s| shown_providers(providers, s)))
    });
    let model_names = Memo::new(move |_| {
        editor
            .settings
            .with(|s| s.models.keys().cloned().collect::<Vec<_>>())
    });
    let adding = RwSignal::new(None::<String>);
    let added_name = NodeRef::<html::Input>::new();
    let add = move |base_url: String| {
        let providers = shown.get_untracked();
        let provider = provider_at(&providers, &base_url).cloned();
        let wanted = added_name
            .get_untracked()
            .map(|input| input.value().trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "model".to_string());
        adding.set(None);
        editor.edit(move |s| {
            let name = unique(&wanted, |name| s.models.contains_key(name));
            s.models.insert(
                name,
                ModelSettings {
                    base_url: provider
                        .as_ref()
                        .map(|p| p.base_url.clone())
                        .unwrap_or_default(),
                    api_key_env: provider.and_then(|p| p.api_key_env),
                    ..ModelSettings::default()
                },
            );
        });
    };
    view! {
        <div class="sd-tab-head">
            <h2>"Models"</h2>
        </div>
        <section class="sd-section">
            <h3>"Providers"</h3>
            <For each=move || shown.get() key=|provider| provider.base_url.clone() let:provider>
                <ProviderCard catalog=catalog provider=provider />
            </For>
        </section>
        <section class="sd-section">
            <h3>"Models"</h3>
            <For each=move || model_names.get() key=Clone::clone let:name>
                <ModelCard editor=editor catalog=catalog providers=shown name=name />
            </For>
            <Show
                when=move || adding.get().is_some()
                fallback=move || view! { <div class="sd-row"><button on:click=move |_| adding.set(Some(String::new()))>"Add Model"</button></div> }
            >
                <div class="sd-card sd-adding">
                    <label class="sd-field">
                        <span>"Name"</span>
                        <input type="text" node_ref=added_name spellcheck="false" placeholder="model" />
                    </label>
                    <p class="sd-note">"Choose its provider:"</p>
                    <div class="sd-row sd-wrap">
                        {move || {
                            shown
                                .get()
                                .into_iter()
                                .map(|provider| {
                                    let base_url = provider.base_url.clone();
                                    view! { <button on:click=move |_| add(base_url.clone())>{provider.name}</button> }
                                })
                                .collect_view()
                        }}
                        <button on:click=move |_| add(String::new())>"Other…"</button>
                        <button on:click=move |_| adding.set(None)>"Cancel"</button>
                    </div>
                </div>
            </Show>
        </section>
    }
}

/// A provider's card: its base URL, its key, ↻, **Check all** for a
/// local one, how far probing has got, and why it could not list its
/// models.
#[component]
fn ProviderCard(catalog: Catalog, provider: Provider) -> impl IntoView {
    let Provider {
        name,
        base_url,
        api_key_env,
    } = provider;
    let local = !is_remote(&base_url);
    let fetched = {
        let base_url = base_url.clone();
        Memo::new(move |_| {
            catalog
                .listings
                .with(|listings| listings.get(&base_url).cloned())
        })
    };
    let progress = {
        let base_url = base_url.clone();
        move || {
            catalog.progress.with(|all| {
                all.get(&base_url)
                    .filter(|p| p.total > 0)
                    .map(Progress::told)
            })
        }
    };
    let refresh = {
        let (base_url, api_key_env) = (base_url.clone(), api_key_env.clone());
        move |_| catalog.fetch(base_url.clone(), api_key_env.clone(), true)
    };
    let check_all = {
        let (base_url, api_key_env) = (base_url.clone(), api_key_env.clone());
        move |_| tauri::check_all(base_url.clone(), api_key_env.clone())
    };
    let key = api_key_env.clone().map(|variable| {
        let (base_url, api_key_env) = (base_url.clone(), api_key_env.clone());
        let fetch = move || catalog.fetch(base_url.clone(), api_key_env.clone(), false);
        view! { <KeyField catalog=catalog variable=variable stored=Callback::new(move |()| fetch()) /> }
    });
    view! {
        <div class="sd-card sd-provider">
            <div class="sd-card-head">
                <h4>{name}</h4>
                <code class="sd-url">{base_url.clone()}</code>
                <button title="List and check its models again" aria-label="Refresh" on:click=refresh>"↻"</button>
                {local.then(|| view! { <button on:click=check_all>"Check all"</button> })}
            </div>
            {key}
            <p class="sd-note">
                {move || {
                    fetched.get().map(|fetched| match fetched.listing {
                        Some(listing) if listing.error.is_none() => format!("{} models", listing.models.len()),
                        _ if fetched.loading => "Loading models…".to_string(),
                        _ => String::new(),
                    })
                }}
                " "
                {progress}
            </p>
            {move || {
                fetched
                    .get()
                    .and_then(|fetched| fetched.listing?.error)
                    .map(|error| view! { <p class="sd-field-error">{error}</p> })
            }}
        </div>
    }
}

/// A key variable's status, and a password field to store a key, with
/// **Save**, and **Remove** when the key is in the keychain. A key is read
/// from its field when **Save** is pressed, and the field is cleared as
/// soon as the keychain answers; `stored` hears that it was stored.
#[component]
fn KeyField(catalog: Catalog, variable: String, stored: Callback<()>) -> impl IntoView {
    catalog.key_status(variable.clone());
    let status = {
        let variable = variable.clone();
        Memo::new(move |_| catalog.keys.with(|keys| keys.get(&variable).cloned()))
    };
    let field = NodeRef::<html::Input>::new();
    let failure = RwSignal::new(None::<String>);
    let save = {
        let variable = variable.clone();
        move |_| {
            let Some(input) = field.get_untracked() else {
                return;
            };
            let variable = variable.clone();
            let key = input.value();
            spawn_local(async move {
                let result = tauri::key_store(variable.clone(), key).await;
                input.set_value("");
                let ok = result.is_ok();
                failure.set(result.err());
                catalog.key_status(variable);
                if ok {
                    stored.run(());
                }
            });
        }
    };
    let remove = {
        let variable = variable.clone();
        move |_| {
            let variable = variable.clone();
            spawn_local(async move {
                failure.set(tauri::key_remove(variable.clone()).await.err());
                catalog.key_status(variable);
            });
        }
    };
    view! {
        <div class="sd-key">
            <code>{variable.clone()}</code>
            <span class="sd-note">{move || status.get().map(|status| status.told())}</span>
        </div>
        <div class="sd-row">
            <input type="password" node_ref=field autocomplete="off" placeholder="API key" />
            <button on:click=save>"Save"</button>
            <Show when=move || status.get() == Some(KeyStatus::InKeychain)>
                <button on:click=remove.clone()>"Remove"</button>
            </Show>
        </div>
        {move || failure.get().map(|failure| view! { <p class="sd-field-error">{failure}</p> })}
    }
}

/// A model's card: its provider, base URL and key variable, its id chosen
/// from the provider's listing, its context window, its probe mark and
/// **Check**.
#[component]
fn ModelCard(
    editor: Editor,
    catalog: Catalog,
    providers: Memo<Vec<Provider>>,
    name: String,
) -> impl IntoView {
    let model = {
        let name = name.clone();
        Memo::new(move |_| {
            editor
                .settings
                .with(|s| s.models.get(&name).cloned().unwrap_or_default())
        })
    };
    let key = StoredValue::new(name.clone());
    let change = move |change: &dyn Fn(&mut ModelSettings)| {
        editor.edit_model(&key.read_value(), change);
    };
    let at = |field: &str| path(&["models", name.as_str(), field]);
    let fetched = Memo::new(move |_| {
        let base_url = model.with(|m| m.base_url.clone());
        catalog
            .listings
            .with(|listings| listings.get(&base_url).cloned())
    });
    let show_all = RwSignal::new(false);
    let list_id = format!(
        "ids-{}",
        name.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    let options = move || {
        let all = show_all.get();
        fetched
            .get()
            .and_then(|fetched| fetched.listing)
            .map(|listing| offered(&listing, all))
            .unwrap_or_default()
            .into_iter()
            .map(|(id, mark)| view! { <option value=id.clone() label=format!("{mark} {id}")></option> })
            .collect_view()
    };
    let provider_value = Signal::derive(move || {
        let base_url = model.with(|m| m.base_url.clone());
        providers
            .with(|providers| provider_at(providers, &base_url).map(|p| p.base_url.clone()))
            .unwrap_or_default()
    });
    let provider_options = Signal::derive(move || {
        let mut options: Vec<(String, String)> = providers
            .get()
            .into_iter()
            .map(|p| (p.base_url, p.name))
            .collect();
        options.push((String::new(), "Other…".to_string()));
        options
    });
    let set_provider = Callback::new(move |base_url: String| {
        let provider =
            providers.with_untracked(|providers| provider_at(providers, &base_url).cloned());
        change(&|m| match &provider {
            Some(provider) => {
                m.base_url.clone_from(&provider.base_url);
                m.api_key_env.clone_from(&provider.api_key_env);
            }
            None => {
                m.base_url.clear();
                m.api_key_env = None;
            }
        });
    });
    let text = |pick: fn(&ModelSettings) -> String| Signal::derive(move || model.with(pick));
    let set_text = |write: fn(&mut ModelSettings, String)| {
        Callback::new(move |value: String| change(&|m| write(m, value.clone())))
    };
    let set_id = Callback::new(move |id: String| {
        let window = fetched.with_untracked(|fetched| {
            let listing = fetched.as_ref()?.listing.as_ref()?;
            listing.models.iter().find(|m| m.id == id)?.context_window
        });
        change(&|m| {
            m.id.clone_from(&id);
            if window.is_some() {
                m.context_window = window;
            }
        });
    });
    let window_at = at("context_window");
    let window_error = editor.error(window_at.clone());
    let set_window = move |event| {
        let window = event_target_value(&event).trim().parse::<u64>().ok();
        change(&|m| m.context_window = window);
    };
    let mark = move || {
        let id = model.with(|m| m.id.clone());
        fetched
            .get()
            .and_then(|fetched| fetched.listing)
            .and_then(|listing| listing.checks.get(&id).cloned())
            .map_or_else(
                || "unchecked".to_string(),
                |check| format!("{} {}", check.mark(), check.told()),
            )
    };
    let checking = RwSignal::new(false);
    let check = move |_| {
        let (base_url, api_key_env, id) =
            model.with_untracked(|m| (m.base_url.clone(), m.api_key_env.clone(), m.id.clone()));
        if id.is_empty() {
            return;
        }
        checking.set(true);
        spawn_local(async move {
            let _ = tauri::check_model(base_url, api_key_env, id).await;
            checking.set(false);
        });
    };
    let loading = move || {
        fetched
            .get()
            .is_some_and(|fetched| fetched.loading && fetched.listing.is_none())
    };
    let removed = name.clone();
    view! {
        <div class="sd-card">
            <div class="sd-card-head">
                <h4>{name.clone()}</h4>
                <span class="sd-mark" title="What checking the model found">{mark}</span>
                <button on:click=check disabled=move || checking.get()>
                    {move || checking.get().then(|| view! { <span class="sd-spinner"></span> })}
                    "Check"
                </button>
                <button on:click=move |_| {
                    let removed = removed.clone();
                    editor.edit(move |s| {
                        s.models.remove(&removed);
                    });
                }>"Remove"</button>
            </div>
            <Choice label="Provider" value=provider_value options=provider_options set=set_provider editor=editor at=at("provider") />
            <TextField label="Base URL" value=text(|m| m.base_url.clone()) set=set_text(|m, v| m.base_url = v) editor=editor at=at("base_url") placeholder="http://localhost:1234/v1" />
            <TextField label="Key variable" value=text(|m| m.api_key_env.clone().unwrap_or_default()) set=set_text(|m, v| m.api_key_env = (!v.trim().is_empty()).then_some(v)) editor=editor at=at("api_key_env") placeholder="none" />
            <TextField
                label="Model id"
                value=text(|m| m.id.clone())
                set=set_id
                editor=editor
                at=at("id")
                list=list_id.clone()
                placeholder=Signal::derive(move || if loading() { "Loading models…".to_string() } else { "Search or type a model id".to_string() })
            />
            <datalist id=list_id>{options}</datalist>
            {move || fetched.get().and_then(|fetched| fetched.listing?.error).map(|error| view! { <p class="sd-field-error">{error}</p> })}
            <label class="sd-check">
                <input type="checkbox" prop:checked=move || show_all.get() on:change=move |event| show_all.set(event_target_checked(&event)) />
                "Show all"
            </label>
            <label class="sd-field">
                <span>"Context window"</span>
                <input
                    type="number"
                    min="1"
                    id=field_id(&window_at)
                    placeholder="the provider's"
                    class:sd-invalid=move || window_error.with(Option::is_some)
                    prop:value=move || model.with(|m| m.context_window.map(|w| w.to_string()).unwrap_or_default())
                    on:input=set_window
                />
            </label>
            <Problem error=window_error />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use social_deduction_stage::settings::Listed;

    #[test]
    fn a_base_url_is_listed_once_it_is_whole() {
        assert!(whole("https://api.openai.com/v1"));
        assert!(whole("http://localhost:1234/v1/"));
        assert!(!whole("http://localhost:1234/"));
        assert!(!whole("localhost:1234/v1"));
        assert!(!whole(""));
    }

    #[test]
    fn a_provider_is_found_by_its_base_url_with_or_without_a_slash() {
        let providers = [Provider {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        }];
        assert_eq!(
            provider_at(&providers, "https://api.openai.com/v1/").map(|p| p.name.as_str()),
            Some("OpenAI")
        );
        assert!(provider_at(&providers, "http://localhost:1234/v1").is_none());
        let mut settings = AppSettings::default();
        settings.models.insert(
            "local".to_string(),
            ModelSettings {
                base_url: "http://localhost:9999/v1".to_string(),
                ..ModelSettings::default()
            },
        );
        settings.models.insert(
            "remote".to_string(),
            ModelSettings {
                base_url: "https://example.com/v1".to_string(),
                ..ModelSettings::default()
            },
        );
        settings
            .models
            .insert("blank".to_string(), ModelSettings::default());
        let shown = shown_providers(&providers, &settings);
        let names: Vec<&str> = shown.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "OpenAI",
                "Local (localhost:9999)",
                "Other (https://example.com/v1)"
            ]
        );
    }

    #[test]
    fn the_dropdown_offers_models_that_play_unless_it_shows_all() {
        let listed = |id: &str, hidden| Listed {
            id: id.to_string(),
            context_window: None,
            hidden,
        };
        let listing = Listing {
            models: vec![
                listed("plays", false),
                listed("unforced", false),
                listed("mute", false),
                listed("unchecked", false),
                listed("embedding", true),
            ],
            error: None,
            checks: BTreeMap::from([
                ("plays".to_string(), Check::Plays),
                ("unforced".to_string(), Check::PlaysUnforced),
                ("mute".to_string(), Check::DidNotPlay("no".to_string())),
            ]),
        };
        let shown = |all| -> Vec<String> {
            offered(&listing, all)
                .into_iter()
                .map(|(id, mark)| format!("{mark} {id}"))
                .collect()
        };
        assert_eq!(shown(false), ["✓ plays", "~ unforced"]);
        assert_eq!(
            shown(true),
            [
                "✓ plays",
                "~ unforced",
                "✗ mute",
                "? unchecked",
                "— embedding"
            ]
        );
    }
}
