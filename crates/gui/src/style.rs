//! The shared stylesheet, `style.css`, on the page: the tokens, `--fa-ink`
//! and the rest, Rich-Chat's `--rc-*` colors and type drawn from them, and
//! the chrome's rules, `.fa-header` and the rest.

use leptos::prelude::*;
use leptos_rich_chat::{Names, RichChatStyle};

/// The shared stylesheet, in the `free-agent` cascade layer. The layer
/// comes after Rich-Chat's, so its rules win over Rich Chat's, and an
/// app's own unlayered rules win over both.
pub const SHEET: &str = concat!(
    "@layer rich-chat, free-agent;\n@layer free-agent {\n",
    include_str!("../style.css"),
    "}\n"
);

/// The narrator's rules: the bubbles of `name` unboxed, centered and in
/// italics, with no name over them. They are unlayered, since they name
/// the app's own narrator, so they win over an app's rules for every
/// bubble.
pub fn narrator(name: &str) -> String {
    let message = format!(".rc-message[data-name=\"{name}\"]");
    format!(
        "{message} .rc-sender {{\n  display: none;\n}}\n\n\
         {message} > .rc-bubble {{\n  border: none;\n  font-size: 16px;\n  font-style: italic;\n  \
         text-align: center;\n  padding-block: 0.1em;\n}}\n"
    )
}

/// The shared stylesheet, with Rich-Chat's under it, where each name's
/// bubbles sit as `names` says, and the bubbles of the `narrator`, when
/// there is one, as a narrator's.
#[component]
pub fn FreeAgentStyle(
    /// Where each name's bubbles sit and their colors.
    #[prop(default = Signal::stored(Names::none()), into)]
    names: Signal<Names>,
    /// The name narration is posted under.
    #[prop(optional)]
    narrator: Option<&'static str>,
) -> impl IntoView {
    view! {
        <RichChatStyle highlight=false fonts=false names=names />
        <style inner_html=SHEET></style>
        {narrator.map(|name| view! { <style inner_html=self::narrator(name)></style> })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_is_layered_after_rich_chat() {
        assert!(SHEET.starts_with("@layer rich-chat, free-agent;\n@layer free-agent {\n"));
        assert!(SHEET.ends_with("}\n"));
    }

    #[test]
    fn the_sheet_holds_the_tokens_light_and_dark_and_no_app_s() {
        // Set once light, and again for each way of choosing dark.
        for (token, set) in [
            ("ink", 3),
            ("paper", 3),
            ("muted", 3),
            ("warn", 1),
            ("serif", 1),
        ] {
            assert_eq!(
                SHEET.matches(&format!("--fa-{token}:")).count(),
                set,
                "{token}"
            );
        }
        assert!(!SHEET.contains("--sd-"));
        assert!(!SHEET.contains(".sd-"));
    }

    #[test]
    fn the_narrator_is_found_by_the_name_the_app_gives() {
        let css = narrator("Narrator");
        assert!(
            css.contains(".rc-message[data-name=\"Narrator\"] .rc-sender {\n  display: none;\n}")
        );
        assert!(
            css.contains(".rc-message[data-name=\"Narrator\"] > .rc-bubble {\n  border: none;")
        );
        assert!(!css.contains("@layer"));
    }
}
