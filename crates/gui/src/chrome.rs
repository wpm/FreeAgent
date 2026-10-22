//! The parts of the chrome every app draws the same way: the start
//! state's head, the select box with its chevron, the × of a remove
//! button, the spinner, and the footer. Chips and the problem mark are
//! the stylesheet's alone: `.fa-chip`, and `.fa-error`, `.fa-warning` and
//! `.fa-field-error`.

use leptos::prelude::*;

/// The commit the page was built from, when the build names one.
const COMMIT: Option<&str> = option_env!("GITHUB_SHA");

/// The shortcut that opens Settings on the platform `platform` names, as
/// `navigator.platform` gives it.
pub(crate) fn settings_shortcut(platform: &str) -> &'static str {
    if platform.starts_with("Mac") {
        "⌘,"
    } else {
        "Ctrl+,"
    }
}

/// The start state's head: the app's `logo`, an SVG, and its `name`, with
/// a link to Settings, which `open_settings` opens, named with its
/// shortcut and helped by `tip`.
#[component]
pub fn StartHead(
    /// The app's mark, an SVG.
    logo: &'static str,
    /// The app's name.
    name: &'static str,
    /// What the link to Settings does.
    #[prop(into)]
    tip: String,
    /// Opens Settings.
    open_settings: Callback<()>,
) -> impl IntoView {
    let platform = window().navigator().platform().unwrap_or_default();
    view! {
        <header class="fa-start-head">
            <div class="fa-logo" inner_html=logo></div>
            <h1>{name}</h1>
            <button class="fa-link fa-open-settings" on:click=move |_| open_settings.run(()) data-tip=tip>
                "Settings " {settings_shortcut(&platform)}
            </button>
        </header>
    }
}

/// A select, the `children`, drawn as a field with a chevron beside it.
/// The select takes the class `fa-select`.
#[component]
pub(crate) fn SelectBox(
    /// The select.
    children: Children,
) -> impl IntoView {
    view! {
        <span class="fa-select-box">
            {children()}
            <svg class="fa-chevron" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
                <path d="M3 4.5l3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
        </span>
    }
}

/// The × of a remove button, a muted stroke the button's `aria-label`
/// names.
#[component]
pub(crate) fn Cross() -> impl IntoView {
    view! {
        <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
            <path d="M3 3l8 8M11 3l-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
        </svg>
    }
}

/// A spinner, turning while something is under way.
#[component]
pub fn Spinner() -> impl IntoView {
    view! { <span class="fa-spinner"></span> }
}

/// The footer: the commit the page was built from, linked, or that it was
/// built locally.
#[component]
pub fn Footer() -> impl IntoView {
    let built = match COMMIT {
        Some(commit) => {
            let href = format!("https://github.com/wpm/FreeAgent/commit/{commit}");
            let short: String = commit.chars().take(7).collect();
            view! { "Built from commit " <a href=href>{short}</a> }.into_any()
        }
        None => view! { "Built locally" }.into_any(),
    };
    view! { <footer class="fa-footer">{built}</footer> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_opens_with_command_comma_on_a_mac_and_control_comma_elsewhere() {
        assert_eq!(settings_shortcut("MacIntel"), "⌘,");
        assert_eq!(settings_shortcut("Win32"), "Ctrl+,");
        assert_eq!(settings_shortcut("Linux x86_64"), "Ctrl+,");
    }
}
