//! The desktop app's settings screen, shown before a game: a preset to
//! play, its configuration, and **Run**.

use leptos::prelude::*;
use leptos_rich_chat::RichChatStyle;

use crate::tauri::Preset;

/// The settings screen: **Preset**, the chosen preset's configuration, and
/// **Run**, with an error under it, in full.
#[component]
pub fn Settings(
    /// The presets **Preset** lists.
    #[prop(into)]
    presets: Signal<Vec<Preset>>,
    /// Which preset is chosen, by its place in `presets`.
    chosen: RwSignal<usize>,
    /// Whether a game is starting: **Run** shows a spinner and is disabled.
    #[prop(into)]
    pending: Signal<bool>,
    /// Why the last game could not start.
    #[prop(into)]
    error: Signal<Option<String>>,
    /// What **Run** calls.
    run: Callback<()>,
) -> impl IntoView {
    let text = move || {
        presets.with(|presets| {
            presets
                .get(chosen.get())
                .map(|preset| preset.text.clone())
                .unwrap_or_default()
        })
    };
    let options = move || {
        presets
            .get()
            .into_iter()
            .enumerate()
            .map(|(index, preset)| {
                view! {
                    <option value=index.to_string() selected=move || chosen.get() == index>
                        {preset.name}
                    </option>
                }
            })
            .collect_view()
    };
    let choose = move |event| {
        if let Ok(index) = event_target_value(&event).parse() {
            chosen.set(index);
        }
    };

    view! {
        <RichChatStyle highlight=false fonts=false />
        <div class="sd-app sd-settings-app">
            <header class="sd-header">
                <h1>"Social Deduction"</h1>
            </header>
            <main class="sd-settings">
                <label class="sd-field">
                    <span>"Preset"</span>
                    <select on:change=choose prop:disabled=pending>{options}</select>
                </label>
                <pre class="sd-config">{text}</pre>
                <div class="sd-run">
                    <button
                        on:click=move |_| run.run(())
                        disabled=move || pending.get() || presets.with(Vec::is_empty)
                    >
                        {move || pending.get().then(|| view! { <span class="sd-spinner"></span> })}
                        "Run"
                    </button>
                </div>
                {move || error.get().map(|error| view! { <pre class="sd-error">{error}</pre> })}
            </main>
        </div>
    }
}
