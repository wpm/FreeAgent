//! The shared stylesheet, `style.css`, on the page: the tokens, `--fa-ink`
//! and the rest, Rich-Chat's `--rc-*` colors and type drawn from them, and
//! the chrome's rules, `.fa-header` and the rest.

use leptos::prelude::*;
use leptos_rich_chat::{Names, RichChatStyle};

/// The shared rules, unlayered as written.
const SHEET: &str = include_str!("../style.css");

/// The cascade layer the shared rules go in. It comes after Rich-Chat's,
/// so they win over Rich Chat's, and an app's own unlayered rules win
/// over both.
pub const LAYER: &str = "free-agent";

/// The shared stylesheet, in [`LAYER`].
pub fn sheet() -> String {
    format!("@layer rich-chat, {LAYER};\n@layer {LAYER} {{\n{SHEET}}}\n")
}

/// The narrator's rules: the bubbles of `name` unboxed, centered and in
/// italics, with no name over them. They are unlayered, since they name
/// the app's own narrator, so they win over an app's rules for every
/// bubble.
pub fn narrator(name: &str) -> String {
    let message = format!(".rc-message[data-name=\"{}\"]", quoted(name));
    format!(
        "{message} .rc-sender {{\n  display: none;\n}}\n\n\
         {message} > .rc-bubble {{\n  border: none;\n  font-size: 16px;\n  font-style: italic;\n  \
         text-align: center;\n  padding-block: 0.1em;\n}}\n"
    )
}

/// `text` as the inside of a CSS string: its backslashes and quotes
/// escaped, and a line break as `\a `.
fn quoted(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '"' => "\\\"".to_string(),
            '\n' => "\\a ".to_string(),
            c => c.to_string(),
        })
        .collect()
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
    #[prop(optional, into)]
    narrator: Option<String>,
) -> impl IntoView {
    view! {
        <RichChatStyle highlight=false fonts=false names=names />
        <style inner_html=sheet()></style>
        {narrator.map(|name| view! { <style inner_html=self::narrator(&name)></style> })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_is_layered_after_rich_chat() {
        let sheet = sheet();
        assert!(sheet.starts_with("@layer rich-chat, free-agent;\n@layer free-agent {\n"));
        assert!(sheet.ends_with("}\n"));
    }

    #[test]
    fn the_sheet_holds_the_tokens_light_and_dark_and_no_app_s() {
        let sheet = sheet();
        // Set once light, and again for each way of choosing dark.
        for (token, set) in [
            ("ink", 3),
            ("paper", 3),
            ("muted", 3),
            ("warn", 1),
            ("serif", 1),
        ] {
            assert_eq!(
                sheet.matches(&format!("--fa-{token}:")).count(),
                set,
                "{token}"
            );
        }
        assert!(!sheet.contains("--sd-"));
        assert!(!sheet.contains(".sd-"));
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

    #[test]
    fn a_narrator_s_name_is_quoted() {
        assert!(narrator("say \"hi\"\\\n").contains("[data-name=\"say \\\"hi\\\"\\\\\\a \"]"));
    }
}
