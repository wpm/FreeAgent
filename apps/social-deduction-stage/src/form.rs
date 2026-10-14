//! The desktop app's settings screen: the configuration file as a form.
//!
//! The backend holds the document and the frontend never parses TOML. It
//! draws a [`FormView`] and sends each edit as a [`Value`] at a TOML path.
//! A key's [`KeyStatus`] and a model's [`Check`] come back the same way,
//! and neither ever carries a key.

use serde::{Deserialize, Serialize};
use std::fmt;

/// What the form shows of a configuration file: every setting it holds,
/// each as the text of its value, `None` when the file leaves it out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormView {
    /// The game the `game` key names: `werewolf` or `avalon`.
    pub game: Option<String>,
    /// The strategy every role not allotted otherwise plays.
    pub strategy: Option<String>,
    /// The game's roles, in the order they are dealt.
    pub roles: Vec<RoleView>,
    /// The game's phase limits, in the order they come.
    pub phases: Vec<Field>,
    /// The `[strategies.*]` tables, by name.
    pub strategies: Vec<StrategyView>,
    /// The `[models.*]` tables, by name.
    pub models: Vec<ModelView>,
    /// The `[text]` blocks, by name.
    pub text: Vec<Field>,
    /// The key variables the models need, by name.
    pub keys: Vec<KeyNeed>,
}

impl FormView {
    /// How many players sit at the table.
    pub fn table_size(&self) -> usize {
        self.roles.iter().map(|role| role.seats).sum()
    }
}

/// One key of a table and its value's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// The key: `day_limit`, or a text block's name.
    pub key: String,
    /// The value's text, `None` when the file leaves it out.
    pub value: Option<String>,
}

/// A `[roles.*]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleView {
    /// The role's key: `werewolf`, `merlin`.
    pub key: String,
    /// The `count` the file gives.
    pub count: Option<String>,
    /// How many seats the role has: the count the file gives, or the
    /// game's.
    pub seats: usize,
    /// The strategy the role's seats play.
    pub strategy: Option<String>,
}

/// A `[strategies.*]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyView {
    /// The strategy's name.
    pub name: String,
    /// `random` or `prompted`.
    pub policy: Option<String>,
    /// The model a prompted strategy asks.
    pub model: Option<String>,
    /// A prompted strategy's system prompt template.
    pub system: Option<String>,
}

/// A `[models.*]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelView {
    /// The model's name.
    pub name: String,
    /// The root of the provider's API.
    pub base_url: Option<String>,
    /// The model's id, as the provider lists it.
    pub id: Option<String>,
    /// The variable holding the API key, over the one known for the
    /// provider.
    pub api_key_env: Option<String>,
    /// The model's context window in tokens.
    pub context_window: Option<String>,
}

/// A key variable and the models that need it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyNeed {
    /// The variable: `OPENAI_API_KEY`.
    pub variable: String,
    /// The models whose key it holds, by name.
    pub models: Vec<String>,
}

/// What an edit sets a key to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Value {
    /// A string.
    Text(String),
    /// An integer.
    Integer(i64),
    /// An empty table, for a new strategy, model or text block.
    Table,
    /// No value: the key is removed, and the game's default applies.
    Remove,
}

/// Where a key variable's key is, never the key itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyStatus {
    /// In the OS keychain.
    InKeychain,
    /// In the environment variable of its name.
    InEnvironment,
    /// Nowhere.
    Missing,
    /// The keychain could not be asked, and why.
    Unavailable(String),
}

impl KeyStatus {
    /// What the Keys section says of a key, the key `models` need.
    pub fn told(&self, models: &[String]) -> String {
        match self {
            KeyStatus::InKeychain => "in the keychain".to_string(),
            KeyStatus::InEnvironment => "in the environment".to_string(),
            KeyStatus::Missing => format!("not set (needed by {})", models.join(", ")),
            KeyStatus::Unavailable(why) => why.clone(),
        }
    }
}

/// What checking a model found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Check {
    /// It plays.
    Plays,
    /// It plays only when not made to call a tool.
    PlaysUnforced,
    /// It did not play, and why.
    DidNotPlay(String),
}

impl fmt::Display for Check {
    /// As a model's card shows it: `✓`, or what is wrong.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Check::Plays => f.write_str("✓"),
            Check::PlaysUnforced => f.write_str("plays only when not made to call a tool"),
            Check::DidNotPlay(why) => f.write_str(why),
        }
    }
}

/// The line an error names in the words `toml` places it with,
/// `at line 5, column 14`, counting from 1.
pub fn error_line(error: &str) -> Option<usize> {
    let (_, after) = error.split_once("at line ")?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Where line `line` of `text`, counting from 1, starts and ends, in
/// UTF-16 code units, as a browser's text area selects it.
pub fn line_range(text: &str, line: usize) -> Option<(u32, u32)> {
    let mut start = 0;
    for (index, content) in text.split('\n').enumerate() {
        let length = content.encode_utf16().count();
        if index + 1 == line {
            return Some((start as u32, (start + length) as u32));
        }
        start += length + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_table_size_is_every_role_s_seats() {
        let role = |key: &str, seats| RoleView {
            key: key.to_string(),
            count: None,
            seats,
            strategy: None,
        };
        let view = FormView {
            roles: vec![role("werewolf", 2), role("villager", 3)],
            ..FormView::default()
        };
        assert_eq!(view.table_size(), 5);
    }

    #[test]
    fn a_value_crosses_as_its_kind_and_its_value() {
        for (value, shown) in [
            (
                Value::Text("45s".into()),
                json!({"kind": "text", "value": "45s"}),
            ),
            (Value::Integer(2), json!({"kind": "integer", "value": 2})),
            (Value::Table, json!({"kind": "table"})),
            (Value::Remove, json!({"kind": "remove"})),
        ] {
            assert_eq!(serde_json::to_value(&value).unwrap(), shown);
            assert_eq!(serde_json::from_value::<Value>(shown).unwrap(), value);
        }
    }

    #[test]
    fn a_key_s_status_says_where_it_is_or_who_needs_it() {
        let models = ["gpt".to_string(), "mini".to_string()];
        assert_eq!(KeyStatus::InKeychain.told(&models), "in the keychain");
        assert_eq!(KeyStatus::InEnvironment.told(&models), "in the environment");
        assert_eq!(
            KeyStatus::Missing.told(&models),
            "not set (needed by gpt, mini)"
        );
        assert_eq!(
            KeyStatus::Unavailable("no store".to_string()).told(&models),
            "no store"
        );
    }

    #[test]
    fn a_check_is_shown_as_a_tick_or_what_is_wrong() {
        assert_eq!(Check::Plays.to_string(), "✓");
        assert_eq!(
            Check::PlaysUnforced.to_string(),
            "plays only when not made to call a tool"
        );
        assert_eq!(Check::DidNotPlay("no".to_string()).to_string(), "no");
    }

    #[test]
    fn an_error_s_line_is_found_where_toml_places_it() {
        let error = "TOML parse error at line 5, column 14\n  |\n5 | day_limit = \"soon\"";
        assert_eq!(error_line(error), Some(5));
        assert_eq!(error_line("strategy everyone names no model"), None);
        assert_eq!(error_line("at line x"), None);
    }

    #[test]
    fn a_line_s_range_counts_utf_16_code_units() {
        let text = "game = \"werewolf\"\n# é😀\nstrategy = \"x\"";
        assert_eq!(line_range(text, 1), Some((0, 17)));
        assert_eq!(line_range(text, 2), Some((18, 23)));
        assert_eq!(line_range(text, 3), Some((24, 38)));
        assert_eq!(line_range(text, 4), None);
    }
}
