//! Asking a model to play, as an app's `models --check` command and its
//! desktop app's **Check** do. The probe is the app's: it sends the
//! requests a player of the app's games would, and says what came of them
//! as [`probed`] does. [`first_answered`] settles the tool choice a model
//! is asked under.

use crate::spec::{Keys, Probed, Spec, required};
use crate::{
    Model, Provider, REQUEST_TIMEOUT, ToolChoice, concerns_the_model, key_variable, refused, served,
};
use anyhow::anyhow;
use free_agent_gui_types::settings::Check;
use std::fmt::Write as _;
use std::io::{self, Write};

/// What asking the model `spec` describes to play finds, with its key
/// looked up in `keys`, as an app's `models --check <id>` asks it with its
/// probe, `plays`.
///
/// # Errors
///
/// When its key is not set, its provider does not serve it, or it could
/// not be asked.
pub async fn check(
    spec: &Spec,
    keys: &dyn Keys,
    plays: impl AsyncFn(&dyn Model, &str) -> anyhow::Result<Probed>,
) -> anyhow::Result<Check> {
    let provider = spec.provider(keys)?;
    provider.listed(&spec.id).await?;
    probe(&provider, &spec.id, plays).await
}

/// What asking `model` to play as the model `id` finds, as the app's
/// probe, `plays`, asks it.
///
/// # Errors
///
/// When the model could not be asked.
pub async fn probe(
    model: &dyn Model,
    id: &str,
    plays: impl AsyncFn(&dyn Model, &str) -> anyhow::Result<Probed>,
) -> anyhow::Result<Check> {
    Ok(match plays(model, id).await? {
        Probed::Played(ToolChoice::Required) => Check::Plays,
        Probed::Played(ToolChoice::Auto) => Check::PlaysUnforced,
        Probed::DidNotPlay(error) => Check::DidNotPlay(format!("{error:#}")),
    })
}

/// How a model is marked in the `models --check` listing: by whether it
/// played when asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Asked to play, it answered every request well, under the tool
    /// choice every request allows.
    Played(ToolChoice),
    /// Asked to play, it did not answer one request well.
    DidNotPlay,
}

/// What an app's `models` command prints of the provider at `base_url`,
/// as [`listing`] lists it: every model it serves, sorted by id, read with
/// the key `keys` holds for the variable [`key_variable`] picks, if any.
/// When there is a `check`, the models it names, or every one when it names
/// none, are marked as [`checked`] marks them, with the reasons going to
/// `reasons`.
///
/// # Errors
///
/// When the key is not set, the provider could not be reached, an id in
/// `check` is not served, or a model could not be asked.
pub async fn models(
    base_url: &str,
    api_key_env: Option<&str>,
    check: Option<&[String]>,
    keys: &dyn Keys,
    reasons: &mut impl Write,
    plays: impl AsyncFn(&dyn Model, &str) -> anyhow::Result<Probed>,
) -> anyhow::Result<String> {
    let api_key = key_variable(base_url, api_key_env)
        .map(|variable| required(keys, variable))
        .transpose()?;
    let provider = Provider::new(base_url, api_key, REQUEST_TIMEOUT)?;
    let listed = provider.listing().await?;
    let mut ids: Vec<String> = listed.iter().map(|model| model.id.clone()).collect();
    ids.sort();
    let models = match check {
        None => ids.into_iter().map(|id| (id, None)).collect(),
        Some(check) => {
            for id in check {
                served(base_url, &listed, id)?;
            }
            checked(&provider, ids, check, reasons, plays).await?
        }
    };
    Ok(listing(&models))
}

/// Each of `ids`, in order, with its mark: those in `check`, or every one
/// of them when `check` is empty, are asked by `model` to play, as the
/// app's probe, `plays`, asks them, one model at a time, since a local
/// server loads each model it is asked for, and the rest are unmarked. Why
/// a model that did not play failed goes to `reasons`, as [`reason`]
/// writes it. A model that could not be asked stops the listing with the
/// error, since nothing it would say about the rest would be true.
pub async fn checked(
    model: &dyn Model,
    ids: Vec<String>,
    check: &[String],
    reasons: &mut impl Write,
    plays: impl AsyncFn(&dyn Model, &str) -> anyhow::Result<Probed>,
) -> anyhow::Result<Vec<(String, Option<Mark>)>> {
    let mut marked = Vec::new();
    for id in ids {
        let mark = if check.is_empty() || check.contains(&id) {
            match plays(model, &id).await? {
                Probed::Played(tool_choice) => Some(Mark::Played(tool_choice)),
                Probed::DidNotPlay(error) => {
                    reason(reasons, &id, &error)?;
                    Some(Mark::DidNotPlay)
                }
            }
        } else {
            None
        };
        marked.push((id, mark));
    }
    Ok(marked)
}

/// Write why the model `id` did not play: a line `! id` naming it as
/// the listing marks it, then `error`'s causes, each on its own indented
/// line, so a provider's words may run to several lines and still be read
/// as one model's.
pub fn reason(out: &mut impl Write, id: &str, error: &anyhow::Error) -> io::Result<()> {
    writeln!(out, "! {id}")?;
    for cause in error.chain() {
        for line in cause.to_string().lines() {
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}

/// What an app's `models` command prints: one id per line, in order. When
/// any model is marked, each id comes after its mark, `* `, `~ ` or `! `,
/// or two spaces when it has none, so the ids line up, and the listing
/// ends, set off by an empty line, with a line saying what the marks mean.
pub fn listing(models: &[(String, Option<Mark>)]) -> String {
    let any_marked = models.iter().any(|(_, mark)| mark.is_some());
    let mut listing = String::new();
    for (id, mark) in models {
        let shown = match mark {
            Some(Mark::Played(ToolChoice::Required)) => "* ",
            Some(Mark::Played(ToolChoice::Auto)) => "~ ",
            Some(Mark::DidNotPlay) => "! ",
            None if any_marked => "  ",
            None => "",
        };
        writeln!(listing, "{shown}{id}").unwrap();
    }
    if any_marked {
        writeln!(
            listing,
            "\n* marks a model that played when asked, ~ one that played when asked to call a \
             tool but not made to, and ! one that did not."
        )
        .unwrap();
    }
    listing
}

/// The tool choice the probe's first request, which `ask` makes under the
/// tool choice it is given, is answered under, and how `ask` came out.
/// The request is first asked under `from`. If that requires a tool call
/// and the provider refuses it as bad, it is asked again allowing one, and
/// both refusals are given if that is refused too.
pub async fn first_answered(
    from: ToolChoice,
    ask: impl AsyncFn(ToolChoice) -> anyhow::Result<()>,
) -> (ToolChoice, anyhow::Result<()>) {
    match ask(from).await {
        Err(required) if from == ToolChoice::Required && refused(&required) => {
            let answered = ask(ToolChoice::Auto).await.map_err(|auto| {
                if concerns_the_model(&auto) {
                    anyhow!("made to call a tool: {required:#}\nasked to call one: {auto:#}")
                } else {
                    auto
                }
            });
            (ToolChoice::Auto, answered)
        }
        answered => (from, answered),
    }
}

/// What the probe of `model_id` for the game named `game` found, its
/// requests asked under `tool_choice`: that the model played, if none
/// `failed`; that it did not play the game, naming the request it did not
/// answer, as `what` the game words it, if the failure concerns the model;
/// and otherwise an error saying the model could not be asked, with the
/// cause beneath it.
///
/// # Errors
///
/// When the model could not be asked.
pub fn probed(
    model_id: &str,
    game: &str,
    tool_choice: ToolChoice,
    failed: Option<(String, anyhow::Error)>,
) -> anyhow::Result<Probed> {
    match failed {
        None => Ok(Probed::Played(tool_choice)),
        Some((what, error)) if concerns_the_model(&error) => Ok(Probed::DidNotPlay(
            error
                .context(format!("did not answer {what}"))
                .context(format!("{model_id} did not play {game}")),
        )),
        Some((_, error)) => Err(error.context(format!("{model_id} could not be asked to play"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canned::{self, refusal};
    use crate::fake::FakeModel;
    use crate::spec::Environment;
    use crate::spec::testing::{LOCAL, qwen};
    use crate::tests::{chosen, first_choice, plays};
    use crate::{Provider, REQUEST_TIMEOUT, Request, Response};
    use std::sync::Mutex;

    /// A model that plays as `id` and refuses as anything else, over two
    /// lines.
    fn playing_only_as(id: &'static str) -> FakeModel {
        FakeModel::answering(move |request| {
            if request.model == id {
                chosen(request)
            } else {
                Err(anyhow!("{}\nin two lines", refusal(&request.model)))
            }
        })
    }

    #[tokio::test]
    async fn every_model_listed_is_checked_unless_ids_are_given_and_then_only_those() {
        let ids = ["gpt-0", "llama", "qwen2.5-7b-instruct"].map(String::from);
        let model = playing_only_as("qwen2.5-7b-instruct");
        let mut reasons = Vec::new();
        let marked = checked(&model, ids.to_vec(), &[], &mut reasons, plays)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [
                ("gpt-0".to_string(), Some(Mark::DidNotPlay)),
                ("llama".to_string(), Some(Mark::DidNotPlay)),
                (
                    "qwen2.5-7b-instruct".to_string(),
                    Some(Mark::Played(ToolChoice::Required))
                ),
            ]
        );
        let asked: Vec<String> = model.requests().into_iter().map(|r| r.model).collect();
        assert_eq!(asked, ids);
        // Each reason under a line naming its model as the listing marks
        // it, the error's causes one per indented line, a cause of several
        // lines indented on each.
        assert_eq!(
            String::from_utf8(reasons).unwrap(),
            "! gpt-0\n  gpt-0 did not play Chess\n  did not answer a selection\n  \
             gpt-0 does not support tools\n  in two lines\n\
             ! llama\n  llama did not play Chess\n  did not answer a selection\n  \
             llama does not support tools\n  in two lines\n"
        );
        // Only the ids given, in the listing's order.
        let model = playing_only_as("qwen2.5-7b-instruct");
        let check = ["qwen2.5-7b-instruct", "gpt-0"].map(String::from);
        let mut reasons = Vec::new();
        let marked = checked(&model, ids.to_vec(), &check, &mut reasons, plays)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [
                ("gpt-0".to_string(), Some(Mark::DidNotPlay)),
                ("llama".to_string(), None),
                (
                    "qwen2.5-7b-instruct".to_string(),
                    Some(Mark::Played(ToolChoice::Required))
                ),
            ]
        );
        let asked: Vec<String> = model.requests().into_iter().map(|r| r.model).collect();
        assert_eq!(asked, ["gpt-0", "qwen2.5-7b-instruct"]);
        assert!(String::from_utf8(reasons).unwrap().starts_with("! gpt-0\n"));
    }

    /// A model asked through a provider, remembering the tool choice each
    /// request to it was made under.
    struct Watched {
        provider: Provider,
        asked: Mutex<Vec<ToolChoice>>,
    }

    #[async_trait::async_trait]
    impl Model for Watched {
        async fn complete(&self, request: &Request) -> anyhow::Result<Response> {
            self.asked.lock().unwrap().push(request.tool_choice);
            self.provider.complete(request).await
        }
    }

    #[tokio::test]
    async fn a_model_that_refuses_to_be_made_to_call_a_tool_is_asked_again_allowing_one() {
        let base_url = canned::refusing_required(first_choice, &["sonnet"]);
        let model = Watched {
            provider: Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap(),
            asked: Mutex::default(),
        };
        let mut reasons = Vec::new();
        let marked = checked(&model, vec!["sonnet".to_string()], &[], &mut reasons, plays)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [("sonnet".to_string(), Some(Mark::Played(ToolChoice::Auto)))]
        );
        assert!(reasons.is_empty());
        assert_eq!(
            model.asked.into_inner().unwrap(),
            [ToolChoice::Required, ToolChoice::Auto]
        );
    }

    #[tokio::test]
    async fn a_model_refusing_both_ways_did_not_play_for_both_refusals() {
        let provider = Provider::new(canned::refusing(&["llama"]), None, REQUEST_TIMEOUT).unwrap();
        let checked = probe(&provider, "llama", plays).await.unwrap();
        let refused = refusal("llama");
        assert!(
            matches!(&checked, Check::DidNotPlay(reason)
                if reason.starts_with("llama did not play Chess: did not answer a selection: made to call a tool: POST ")
                    && reason.contains("\nasked to call one: POST ")
                    && reason.ends_with(&refused)),
            "{checked:?}"
        );
    }

    #[tokio::test]
    async fn a_model_refused_one_way_then_not_reached_the_other_could_not_be_asked() {
        let base_url = canned::refusing_required_then_busy(&["llama"]);
        let provider = Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = plays(&provider, "llama").await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with("llama could not be asked to play: POST "),
            "{shown}"
        );
        assert!(shown.contains("429 Too Many Requests"), "{shown}");
    }

    #[tokio::test]
    async fn a_model_that_could_not_be_asked_stops_the_listing_with_the_error() {
        // A provider answering anything but the model's answer says nothing
        // about this model or the rest.
        let (base_url, _served) = canned::answering("429 Too Many Requests", "{}");
        let provider = Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap();
        let ids = ["gpt-0", "llama"].map(String::from);
        let mut reasons = Vec::new();
        let error = checked(&provider, ids.to_vec(), &[], &mut reasons, plays)
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with("gpt-0 could not be asked to play"),
            "{shown}"
        );
        assert!(shown.contains("429 Too Many Requests"), "{shown}");
        assert!(reasons.is_empty());
    }

    #[tokio::test]
    async fn checking_a_model_says_whether_it_plays_and_whether_it_must_be_made_to() {
        let id = "qwen2.5-7b-instruct";
        let spec = qwen(&canned::playing(first_choice, &[id]), "");
        assert_eq!(
            check(&spec, &Environment, plays).await.unwrap(),
            Check::Plays
        );
        let spec = qwen(&canned::refusing_required(first_choice, &[id]), "");
        assert_eq!(
            check(&spec, &Environment, plays).await.unwrap(),
            Check::PlaysUnforced
        );
        let spec = qwen(&canned::playing_only(first_choice, &[id], &[]), "");
        let checked = check(&spec, &Environment, plays).await.unwrap();
        assert!(
            matches!(&checked, Check::DidNotPlay(reason)
                if reason.starts_with("qwen2.5-7b-instruct did not play Chess")),
            "{checked:?}"
        );
    }

    #[tokio::test]
    async fn a_model_not_served_or_without_its_key_is_an_error() {
        let spec = qwen(&canned::serving(&["llama"]), "");
        let error = check(&spec, &Environment, plays).await.unwrap_err();
        assert!(
            format!("{error}").contains("does not serve a model with the id qwen2.5-7b-instruct")
        );
        let spec = qwen(LOCAL, "api_key_env = \"FREE_AGENT_NO_SUCH_KEY\"");
        let error = check(&spec, &Environment, plays).await.unwrap_err();
        assert_eq!(format!("{error}"), "FREE_AGENT_NO_SUCH_KEY is not set");
    }

    #[tokio::test]
    async fn models_lists_the_provider_s_models_sorted_and_marks_those_checked() {
        let base_url = canned::playing_only(first_choice, &["zephyr", "llama"], &["llama"]);
        let mut reasons = Vec::new();
        let plain = models(&base_url, None, None, &Environment, &mut reasons, plays)
            .await
            .unwrap();
        assert_eq!(plain, "llama\nzephyr\n");
        let check = ["llama".to_string()];
        let marked = models(
            &base_url,
            None,
            Some(&check),
            &Environment,
            &mut reasons,
            plays,
        )
        .await
        .unwrap();
        assert!(marked.starts_with("* llama\n  zephyr\n\n"), "{marked}");
        assert!(reasons.is_empty());
        let unserved = ["gpt-0".to_string()];
        let error = models(
            &base_url,
            None,
            Some(&unserved),
            &Environment,
            &mut reasons,
            plays,
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("gpt-0"), "{error:#}");
    }

    #[tokio::test]
    async fn models_refuses_a_key_variable_that_is_not_set_before_asking_anyone() {
        let error = models(
            &canned::unreachable(),
            Some("FREE_AGENT_NO_SUCH_KEY"),
            None,
            &Environment,
            &mut Vec::new(),
            plays,
        )
        .await
        .unwrap_err();
        assert_eq!(format!("{error}"), "FREE_AGENT_NO_SUCH_KEY is not set");
    }

    #[test]
    fn the_listing_is_one_id_per_line_and_when_any_is_marked_lines_them_up_and_says_what_the_marks_mean()
     {
        let ids = ["gpt-0", "qwen2.5-7b-instruct"].map(String::from);
        let plain: Vec<(String, Option<Mark>)> = ids.iter().map(|id| (id.clone(), None)).collect();
        assert_eq!(listing(&plain), "gpt-0\nqwen2.5-7b-instruct\n");
        assert_eq!(listing(&[]), "");
        let models = [
            ("gpt-0".to_string(), Some(Mark::DidNotPlay)),
            ("llama".to_string(), None),
            (
                "qwen2.5-7b-instruct".to_string(),
                Some(Mark::Played(ToolChoice::Required)),
            ),
            ("sonnet".to_string(), Some(Mark::Played(ToolChoice::Auto))),
        ];
        assert_eq!(
            listing(&models),
            "! gpt-0\n  llama\n* qwen2.5-7b-instruct\n~ sonnet\n\n\
             * marks a model that played when asked, ~ one that played when asked to call a tool \
             but not made to, and ! one that did not.\n"
        );
    }
}
