//! A model's context window: how many tokens it takes in at once, and
//! where that number came from. Every game settles a model's window the
//! same way, against what its own largest request needs.

use super::spec::Spec;
use super::{CONTEXT_WINDOW, known_context_window};
use anyhow::bail;
use serde::{Deserialize, Serialize};

/// A model's context window, in tokens, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextWindow {
    /// How many tokens the model takes in at once.
    pub tokens: u64,
    /// Where the window came from.
    pub source: Source,
}

/// Where a context window came from, written as `file`, `provider`,
/// `provider default` or `default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The configuration file.
    File,
    /// The provider.
    Provider,
    /// What `src/models.toml` knows of the provider.
    #[serde(rename = "provider default")]
    ProviderDefault,
    /// The code's default.
    Default,
}

/// The context window of a model of the provider at `base_url` whose
/// configuration gives none: `reported`, the one the provider's listing
/// reports for the model, otherwise the provider's default in
/// `src/models.toml`, otherwise [`CONTEXT_WINDOW`].
pub fn unset(base_url: &str, reported: Option<u64>) -> ContextWindow {
    let given = |tokens, source| ContextWindow { tokens, source };
    reported
        .map(|tokens| given(tokens, Source::Provider))
        .or(known_context_window(base_url).map(|tokens| given(tokens, Source::ProviderDefault)))
        .unwrap_or(given(CONTEXT_WINDOW, Source::Default))
}

/// The context window of the model `name`, which `spec` describes: the one
/// its table gives, otherwise the one [`unset`] gives. A window smaller
/// than `needed`, the tokens the game's largest request to the model
/// takes before what the player remembers, by estimate, is an error naming
/// the model and where the window came from.
pub fn settle(
    name: &str,
    spec: &Spec,
    reported: Option<u64>,
    needed: u64,
) -> anyhow::Result<ContextWindow> {
    let window = match spec.context_window {
        Some(tokens) => ContextWindow {
            tokens,
            source: Source::File,
        },
        None => unset(&spec.base_url, reported),
    };
    if window.tokens < needed {
        let setting = match window.source {
            Source::File => format!("[models.{name}] context_window"),
            Source::Provider => format!("the context window the provider reports for {name}"),
            Source::ProviderDefault => {
                format!("the provider's default context window for {name}")
            }
            Source::Default => format!("the default context window for {name}"),
        };
        let remedy = match window.source {
            Source::File => String::new(),
            _ => format!("; context_window in [models.{name}] gives the window"),
        };
        bail!(
            "{setting} is {} tokens, smaller than the {needed} a request takes before its story, by estimate{remedy}",
            window.tokens
        );
    }
    Ok(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::spec::tests::{LOCAL, OPENAI, qwen};

    #[test]
    fn the_context_window_falls_back_from_the_file_to_the_listing_to_the_provider_to_the_code() {
        let window = |tokens, source| ContextWindow { tokens, source };
        let settled = |spec: &Spec, reported| settle("qwen", spec, reported, 0).unwrap();
        let file = qwen(LOCAL, "context_window = 8192");
        assert_eq!(settled(&file, Some(32768)), window(8192, Source::File));
        let plain = qwen(LOCAL, "");
        assert_eq!(
            settled(&plain, Some(32768)),
            window(32768, Source::Provider)
        );
        assert_eq!(
            settled(&plain, None),
            window(CONTEXT_WINDOW, Source::Default)
        );
        assert_eq!(CONTEXT_WINDOW, 4096);
        // A provider `src/models.toml` gives a window for.
        let openai = qwen(OPENAI, "");
        assert_eq!(
            settled(&openai, Some(32768)),
            window(32768, Source::Provider)
        );
        assert_eq!(
            settled(&openai, None),
            window(128000, Source::ProviderDefault)
        );
    }

    #[test]
    fn a_window_as_large_as_needed_is_settled_and_a_smaller_one_is_an_error_naming_the_model_and_where_it_came_from()
     {
        let plain = qwen(LOCAL, "");
        assert_eq!(
            settle("qwen", &plain, Some(5000), 5000).unwrap(),
            ContextWindow {
                tokens: 5000,
                source: Source::Provider,
            }
        );
        let refused = |spec: &Spec, reported| {
            let error = settle("qwen", spec, reported, 5000).unwrap_err();
            format!("{error:#}")
        };
        let too_small = "smaller than the 5000 a request takes before its story, by estimate";
        let remedy = "; context_window in [models.qwen] gives the window";
        assert_eq!(
            refused(&plain, Some(4999)),
            format!(
                "the context window the provider reports for qwen is 4999 tokens, {too_small}{remedy}"
            )
        );
        assert_eq!(
            refused(&plain, None),
            format!("the default context window for qwen is 4096 tokens, {too_small}{remedy}")
        );
        let error = settle("qwen", &qwen(OPENAI, ""), None, 200_000).unwrap_err();
        assert!(
            format!("{error:#}")
                .starts_with("the provider's default context window for qwen is 128000 tokens"),
            "{error:#}"
        );
        assert_eq!(
            refused(&qwen(LOCAL, "context_window = 2000"), None),
            format!("[models.qwen] context_window is 2000 tokens, {too_small}")
        );
    }
}
