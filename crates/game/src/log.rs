//! What every game's log shares: how an episode [`Ended`], and the
//! secrets it never carries. A log writes every key the run loaded, its
//! [`Secrets`], as [`REDACTED`], and every URL without its userinfo and
//! query.

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

/// The last line of every episode: how its run came back. An episode with
/// no `Ended` line is cut off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    /// A side won.
    Won,
    /// The episode was stopped before anyone won.
    Stopped,
    /// The episode failed, with the error written as it says itself,
    /// causes included.
    Failed(String),
}

impl Ended {
    /// How an episode whose run came back `ran` ended, a side having won
    /// when `won`. A failure is a failure even after a win.
    pub fn of(ran: &anyhow::Result<()>, won: bool) -> Self {
        match ran {
            Err(error) => Ended::Failed(format!("{error:#}")),
            Ok(()) if won => Ended::Won,
            Ok(()) => Ended::Stopped,
        }
    }

    /// How an episode ended, read from `line`, its log's last line.
    ///
    /// # Errors
    ///
    /// Fails when `line` is not how an episode ended.
    pub fn read(line: &str) -> serde_json::Result<Self> {
        /// The last line of a log, its payload alone.
        #[derive(Deserialize)]
        struct Last {
            payload: Payload,
        }
        /// The last line's payload.
        #[derive(Deserialize)]
        enum Payload {
            Ended(Ended),
        }
        let Last {
            payload: Payload::Ended(ended),
        } = serde_json::from_str(line)?;
        Ok(ended)
    }
}

/// What a log writes in place of a secret.
pub const REDACTED: &str = "[redacted]";

/// The keys a run loaded, which its log never carries.
#[derive(Debug, Clone, Default)]
pub struct Secrets(
    /// Each key as it is written inside a JSON string, escaped.
    Vec<SecretString>,
);

impl FromIterator<SecretString> for Secrets {
    /// The secrets `keys` are, each once. An empty key is no secret.
    fn from_iter<I: IntoIterator<Item = SecretString>>(keys: I) -> Self {
        let mut escaped: Vec<String> = keys
            .into_iter()
            .filter(|key| !key.expose_secret().is_empty())
            .map(|key| {
                let quoted = serde_json::Value::from(key.expose_secret()).to_string();
                quoted[1..quoted.len() - 1].to_string()
            })
            .collect();
        escaped.sort();
        escaped.dedup();
        Secrets(escaped.into_iter().map(SecretString::from).collect())
    }
}

impl Secrets {
    /// `line`, a line of JSON, with every secret replaced by [`REDACTED`]
    /// and every URL written [`without_credentials`].
    pub fn redacted(&self, line: &str) -> String {
        let mut line = line.to_string();
        for secret in &self.0 {
            if line.contains(secret.expose_secret()) {
                line = line.replace(secret.expose_secret(), REDACTED);
            }
        }
        without_credentials(&line)
    }
}

/// `text` with every `http` or `https` URL in it written without its
/// userinfo, its query or its fragment: `https://user:pass@host/v1?key=abc`
/// is written `https://host/v1`. A URL ends at a space, a quote, a
/// backslash, an angle bracket, a parenthesis or a brace.
pub fn without_credentials(text: &str) -> String {
    let mut written = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        let (before, from) = rest.split_at(start);
        let end = from
            .find(|c: char| c.is_whitespace() || "\"'`\\<>(){}".contains(c))
            .unwrap_or(from.len());
        let (url, after) = from.split_at(end);
        written.push_str(before);
        written.push_str(&stripped(url));
        rest = after;
    }
    written.push_str(rest);
    written
}

/// `url`, which starts with its scheme, without its userinfo, query and
/// fragment.
fn stripped(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or_default();
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    format!("{scheme}://{host}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_that_fails_ends_failed_one_that_comes_back_after_a_win_won_and_otherwise_stopped() {
        let failed = Err(anyhow::anyhow!("no model").context("actor Player 1 failed"));
        assert_eq!(
            Ended::of(&failed, true),
            Ended::Failed("actor Player 1 failed: no model".to_string())
        );
        assert_eq!(Ended::of(&Ok(()), true), Ended::Won);
        assert_eq!(Ended::of(&Ok(()), false), Ended::Stopped);
    }

    #[test]
    fn how_an_episode_ended_is_read_from_its_last_line_and_any_other_line_is_an_error() {
        let last = r#"{"time":"2026-10-21T00:00:00Z","episode":"x","payload":{"Ended":{"failed":"no model"}}}"#;
        assert_eq!(
            Ended::read(last).unwrap(),
            Ended::Failed("no model".to_string())
        );
        assert!(Ended::read(r#"{"payload":{"Thinking":{}}}"#).is_err());
    }

    /// The secrets `keys` are.
    fn secrets(keys: &[&str]) -> Secrets {
        keys.iter().copied().map(SecretString::from).collect()
    }

    /// How an episode that failed with `error` is written in a line of a
    /// log with `secrets`.
    fn failed_with(secrets: &Secrets, error: &str) -> String {
        let ended = serde_json::to_string(&Ended::Failed(error.to_string())).unwrap();
        secrets.redacted(&ended)
    }

    #[test]
    fn how_an_episode_ended_is_written_as_a_reader_of_any_game_reads_it() {
        let written = |ended| serde_json::to_value(ended).unwrap();
        assert_eq!(written(Ended::Won), serde_json::json!("won"));
        assert_eq!(written(Ended::Stopped), serde_json::json!("stopped"));
        assert_eq!(
            written(Ended::Failed("no model".to_string())),
            serde_json::json!({"failed": "no model"})
        );
    }

    #[test]
    fn every_secret_is_written_redacted_and_the_line_still_reads() {
        let written = failed_with(
            &secrets(&["sk-one", "sk-two", "sk-one", ""]),
            "sk-one was refused, as was sk-two, and then sk-one again",
        );
        assert!(!written.contains("sk-one") && !written.contains("sk-two"));
        assert_eq!(
            serde_json::from_str::<Ended>(&written).unwrap(),
            Ended::Failed(
                "[redacted] was refused, as was [redacted], and then [redacted] again".to_string()
            )
        );
    }

    #[test]
    fn a_secret_with_characters_json_escapes_is_redacted() {
        let key = "sk-\"quoted\"\\back\tslash\u{1}";
        let written = failed_with(&secrets(&[key]), &format!("the key {key} was refused"));
        assert!(
            written.contains("the key [redacted] was refused"),
            "{written}"
        );
        let escaped = serde_json::Value::from(key).to_string();
        assert!(
            !written.contains(&escaped[1..escaped.len() - 1]),
            "{written}"
        );
    }

    #[test]
    fn a_line_is_written_with_its_urls_without_credentials() {
        assert_eq!(
            Secrets::default().redacted("\"https://u:p@host/v1?key=k\""),
            "\"https://host/v1\""
        );
    }

    #[test]
    fn the_secrets_are_never_shown() {
        assert!(!format!("{:?}", secrets(&["sk-shown"])).contains("sk-shown"));
    }

    #[test]
    fn a_url_is_written_without_its_userinfo_or_its_query() {
        for (url, written) in [
            ("https://user:pass@host/v1", "https://host/v1"),
            ("https://host/v1?key=abc", "https://host/v1"),
            ("https://user:pass@host/v1?key=abc", "https://host/v1"),
            ("https://host/v1#key=abc", "https://host/v1"),
            ("http://localhost:1234/v1", "http://localhost:1234/v1"),
            (
                "http://token@localhost:1234?key=a@b",
                "http://localhost:1234",
            ),
            ("https://host/models/a@b", "https://host/models/a@b"),
            (
                "http://u:p@[::1]:1234/v1?key=[redacted]",
                "http://[::1]:1234/v1",
            ),
        ] {
            assert_eq!(without_credentials(url), written, "{url}");
        }
    }

    #[test]
    fn every_url_in_a_text_is_written_without_credentials_and_the_rest_as_it_was() {
        assert_eq!(
            without_credentials(
                "POST https://u:p@a.example/v1/chat?key=k failed \
                 (error sending request for url (http://b.example/v1?key=k)): \
                 \"https://c.example/?q=1\"; no URL here"
            ),
            "POST https://a.example/v1/chat failed \
             (error sending request for url (http://b.example/v1)): \
             \"https://c.example/\"; no URL here"
        );
    }
}
