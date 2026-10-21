//! What every app's settings share: the look, the models and the providers
//! they are reached through, and the errors a field can have. The backend
//! stores the settings and checks them, and the Settings window edits
//! them, so they cross between the two as they are. The models each
//! provider lists, what probing a model found and where an API key is
//! cross the same way. None of them ever carries a key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `wanted`, or, when `taken` says it is taken, `wanted-2` or the next
/// number not taken.
pub fn unique(wanted: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut name = wanted.to_string();
    let mut number = 1;
    while taken(&name) {
        number += 1;
        name = format!("{wanted}-{number}");
    }
    name
}

/// A key as words, the first capitalized: `day_limit` as `Day limit`,
/// `merlin` as `Merlin`.
pub fn label(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Whether `name` can name a prompt part: words of letters, digits and
/// underscores with one space between them, not starting with a digit, so
/// that a template can pull it in with its [`part_variable`].
pub fn is_part_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_')
        && name.split(' ').all(|word| {
            !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The variable a template pulls the prompt part `name` in with: its name
/// with each space an underscore, so `House rules` is `{{ House_rules }}`.
pub fn part_variable(name: &str) -> String {
    name.replace(' ', "_")
}

/// What belongs to no game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// How the app looks.
    pub appearance: Appearance,
}

/// How the app looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// As the system looks, and as the game shows itself where it has a
    /// look of its own.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl Appearance {
    /// The page's `data-theme`, which the look holds to whatever the game
    /// shows, or `None` for the system's.
    pub fn theme(self) -> Option<&'static str> {
        match self {
            Appearance::System => None,
            Appearance::Light => Some("light"),
            Appearance::Dark => Some("dark"),
        }
    }
}

/// One model from one provider, and how to reach it: a configuration
/// file's `[models.<name>]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettings {
    /// The root of an OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The model's id, exactly as the provider lists it.
    pub id: String,
    /// The variable holding the API key, over the one known for the
    /// provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// The model's context window, in tokens, over what the provider
    /// reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// How long a request may take, such as `60s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout: Option<String>,
}

/// What is wrong with one field of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// Where the field is: `["models", "gpt", "id"]`.
    pub path: Vec<String>,
    /// What is wrong, in a sentence that stands alone.
    pub message: String,
}

impl FieldError {
    /// The error `message` at `path`.
    pub fn new(path: &[&str], message: impl Into<String>) -> Self {
        Self {
            path: path.iter().map(ToString::to_string).collect(),
            message: message.into(),
        }
    }

    /// Whether the error is at `path`.
    pub fn is_at(&self, path: &[String]) -> bool {
        self.path == path
    }
}

/// A provider a model may be reached through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// What the app calls it: `OpenAI`, `Local (localhost:1234)`.
    pub name: String,
    /// The root of its OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The variable that conventionally holds its API key, if it wants one.
    pub api_key_env: Option<String>,
}

impl Provider {
    /// The local server on `port`: `Local (localhost:1234)`, wanting no
    /// key.
    pub fn local(port: u16) -> Self {
        Self {
            name: format!("Local (localhost:{port})"),
            base_url: format!("http://localhost:{port}/v1"),
            api_key_env: None,
        }
    }

    /// The port of the local server at `base_url`, if it is one.
    pub fn local_port(base_url: &str) -> Option<u16> {
        let rest = base_url.trim().strip_prefix("http://localhost:")?;
        rest.trim_end_matches('/').strip_suffix("/v1")?.parse().ok()
    }
}

/// The provider of `providers` at `base_url`, with or without a final
/// slash, if there is one.
pub fn provider_at<'a>(providers: &'a [Provider], base_url: &str) -> Option<&'a Provider> {
    providers
        .iter()
        .find(|provider| root(&provider.base_url) == root(base_url))
}

/// The host of `base_url`: `api.example.com` of
/// `https://api.example.com/v1`.
pub fn host(base_url: &str) -> &str {
    let base_url = base_url.trim();
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

/// `base_url` as base URLs are compared: without spaces around it or a
/// final slash.
pub fn root(base_url: &str) -> &str {
    base_url.trim().trim_end_matches('/')
}

/// Whether `base_url` is whole: an `http` or `https` URL ending in `/v1`.
pub fn whole(base_url: &str) -> bool {
    let base_url = root(base_url);
    (base_url.starts_with("http://") || base_url.starts_with("https://"))
        && base_url.ends_with("/v1")
}

/// Whether the provider at `base_url` is remote: reached over `https`.
pub fn is_remote(base_url: &str) -> bool {
    base_url.trim_start().starts_with("https://")
}

/// The models a provider lists, or why it could not list them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Listing {
    /// Each model the provider serves, in the order it lists them.
    pub models: Vec<Listed>,
    /// Why the provider could not list its models.
    pub error: Option<String>,
    /// What probing found of each model, by id.
    pub checks: BTreeMap<String, Check>,
}

/// One model a provider lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// Its id.
    pub id: String,
    /// The context window a game gives it unless its settings give one, in
    /// tokens: the one the provider reports, or the provider's default.
    pub context_window: u64,
    /// Whether its id says it is not a model that talks, such as an
    /// embedding model, which is never probed.
    pub hidden: bool,
}

/// What probing one model found, or `None` for a probe that could not be
/// run, sent as each probe ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// The provider's base URL.
    pub base_url: String,
    /// The model's id.
    pub id: String,
    /// What the probe found.
    pub check: Option<Check>,
}

/// How far probing a provider's models has got, sent as each probe ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The provider's base URL.
    pub base_url: String,
    /// How many models have been probed.
    pub checked: usize,
    /// How many models there are to probe: those not probed once the
    /// account failed are left for the next round.
    pub total: usize,
    /// Why the last of its models that could not be asked could not be,
    /// if one could not be.
    #[serde(default)]
    pub unchecked: Option<Unchecked>,
}

/// Why a model could not be asked to play, which says nothing about the
/// model, and what the provider said, or the error when it said nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unchecked {
    /// What the trouble was.
    pub trouble: Trouble,
    /// What the provider said, or the error.
    pub said: String,
}

/// What kept a model from being asked to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trouble {
    /// The account has no credits left.
    NoCredits,
    /// The provider did not accept the API key.
    KeyRefused,
    /// The API key is not allowed to use the provider's models.
    KeyForbidden,
    /// Anything else, such as a provider that could not be reached.
    Failed,
}

impl Unchecked {
    /// Whether every model of the provider would fail to be asked the
    /// same way, the account being the trouble.
    pub fn account_wide(&self) -> bool {
        self.trouble != Trouble::Failed
    }
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
    /// What a provider says of its key.
    pub fn told(&self) -> String {
        match self {
            KeyStatus::InKeychain => "in the keychain".to_string(),
            KeyStatus::InEnvironment => "from the environment".to_string(),
            KeyStatus::Missing => "not set".to_string(),
            KeyStatus::Unavailable(why) => why.clone(),
        }
    }

    /// Whether there is a key to send.
    pub fn is_set(&self) -> bool {
        matches!(self, KeyStatus::InKeychain | KeyStatus::InEnvironment)
    }
}

/// What asking a model to play found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Check {
    /// It plays.
    Plays,
    /// It plays only when not made to call a tool.
    PlaysUnforced,
    /// It did not play, and why.
    DidNotPlay(String),
}

impl Check {
    /// Whether the model plays, made to call a tool or not.
    pub fn plays(&self) -> bool {
        !matches!(self, Check::DidNotPlay(_))
    }

    /// Why the model did not play, if it did not: the cause beneath the
    /// request it did not answer, as its probe words the reason, or all
    /// of the reason.
    pub fn cause(&self) -> Option<&str> {
        let Check::DidNotPlay(reason) = self else {
            return None;
        };
        let cause = reason
            .split_once(": did not answer ")
            .and_then(|(_, rest)| rest.split_once(": "))
            .map_or(reason.as_str(), |(_, cause)| cause);
        Some(cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_look_holds_the_theme_or_leaves_it_to_the_system() {
        assert_eq!(Appearance::System.theme(), None);
        assert_eq!(Appearance::Light.theme(), Some("light"));
        assert_eq!(Appearance::Dark.theme(), Some("dark"));
        assert_eq!(General::default().appearance, Appearance::System);
    }

    #[test]
    fn a_field_error_is_at_its_path() {
        let error = FieldError::new(&["models", "gpt", "id"], "no id");
        assert_eq!(error.message, "no id");
        assert!(error.is_at(&["models", "gpt", "id"].map(String::from)));
        assert!(!error.is_at(&["models".to_string()]));
    }

    #[test]
    fn a_provider_is_found_by_its_base_url_with_or_without_a_final_slash() {
        let openai = Provider {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        };
        let providers = std::slice::from_ref(&openai);
        assert_eq!(
            provider_at(providers, " https://api.openai.com/v1/").map(|p| p.name.as_str()),
            Some("OpenAI")
        );
        assert_eq!(provider_at(providers, "https://api.example.com/v1"), None);
        assert_eq!(
            root(" https://api.openai.com/v1/ "),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn a_host_is_the_base_url_s_between_its_scheme_and_its_path() {
        assert_eq!(host(" https://api.example.com/v1/ "), "api.example.com");
        assert_eq!(host("localhost:1234/v1"), "localhost:1234");
        assert_eq!(host("localhost"), "localhost");
    }

    #[test]
    fn a_base_url_is_whole_when_it_is_http_or_https_and_ends_in_v1() {
        assert!(whole("https://api.openai.com/v1"));
        assert!(whole(" http://localhost:1234/v1/ "));
        assert!(!whole("http://localhost:1234/"));
        assert!(!whole("localhost:1234/v1"));
        assert!(!whole(""));
    }

    #[test]
    fn a_key_s_status_says_where_it_is() {
        assert_eq!(KeyStatus::InKeychain.told(), "in the keychain");
        assert_eq!(KeyStatus::InEnvironment.told(), "from the environment");
        assert_eq!(KeyStatus::Missing.told(), "not set");
        assert_eq!(
            KeyStatus::Unavailable("no store".to_string()).told(),
            "no store"
        );
        assert!(KeyStatus::InKeychain.is_set() && KeyStatus::InEnvironment.is_set());
        assert!(!KeyStatus::Missing.is_set());
    }

    #[test]
    fn a_model_plays_whether_or_not_it_is_made_to_call_a_tool() {
        let no = Check::DidNotPlay("no tools".to_string());
        assert!(Check::Plays.plays() && Check::PlaysUnforced.plays() && !no.plays());
        assert_eq!(no.cause(), Some("no tools"));
        assert_eq!(Check::Plays.cause(), None);
        let reason = "qwen did not play Chess: did not answer as a player: POST x: no: tools";
        assert_eq!(
            Check::DidNotPlay(reason.to_string()).cause(),
            Some("POST x: no: tools")
        );
    }

    #[test]
    fn only_a_failure_of_the_account_stops_a_provider_s_checks() {
        let unchecked = |trouble| Unchecked {
            trouble,
            said: String::new(),
        };
        for trouble in [
            Trouble::NoCredits,
            Trouble::KeyRefused,
            Trouble::KeyForbidden,
        ] {
            assert!(unchecked(trouble).account_wide(), "{trouble:?}");
        }
        assert!(!unchecked(Trouble::Failed).account_wide());
    }

    #[test]
    fn a_prompt_part_s_name_is_words_of_letters_digits_and_underscores() {
        for name in [
            "rules",
            "tone_2",
            "_aside",
            "Rules",
            "House rules",
            "the 2nd rule",
        ] {
            assert!(is_part_name(name), "{name}");
        }
        for name in [
            "",
            "2nd",
            " rules",
            "rules ",
            "the  rules",
            "tone-2",
            "règles",
        ] {
            assert!(!is_part_name(name), "{name}");
        }
    }

    #[test]
    fn a_prompt_part_is_pulled_in_by_its_name_with_underscores_for_spaces() {
        assert_eq!(part_variable("rules"), "rules");
        assert_eq!(part_variable("House rules"), "House_rules");
    }

    #[test]
    fn a_name_taken_is_numbered() {
        let taken = ["gpt", "gpt-2"];
        assert_eq!(unique("gpt", |name| taken.contains(&name)), "gpt-3");
        assert_eq!(unique("claude", |name| taken.contains(&name)), "claude");
    }

    #[test]
    fn a_key_is_labeled_as_words() {
        assert_eq!(label("day_limit"), "Day limit");
        assert_eq!(label("merlin"), "Merlin");
        assert_eq!(label(""), "");
    }

    #[test]
    fn a_local_server_is_named_for_its_port_and_known_by_its_base_url() {
        let local = Provider::local(1234);
        assert_eq!(local.name, "Local (localhost:1234)");
        assert_eq!(local.base_url, "http://localhost:1234/v1");
        assert_eq!(local.api_key_env, None);
        assert_eq!(
            Provider::local_port("http://localhost:1234/v1/"),
            Some(1234)
        );
        assert_eq!(Provider::local_port("http://localhost:x/v1"), None);
        assert_eq!(Provider::local_port("http://localhost:1234"), None);
        assert_eq!(Provider::local_port("https://api.openai.com/v1"), None);
        assert!(is_remote(" https://api.openai.com/v1"));
        assert!(!is_remote("http://localhost:1234/v1"));
    }
}
