//! Asking a model to play, as the `models --check` command and the desktop
//! app's **Check** do: each game's probe, in turn, sends the requests a
//! player would.

use super::spec::{Keys, Probed, Spec};
use super::{Model, ToolChoice};
use crate::config::Name;
use social_deduction_stage::settings::Check;
use std::io::{self, Write};

/// What asking `model` to play each game in [`Name::ALL`] as the model
/// `id` finds. Each game's probe starts from the tool choice the games
/// before it settled, and a model that does not play one game is not
/// asked to play the next.
///
/// # Errors
///
/// When the model could not be asked.
pub async fn plays(model: &dyn Model, id: &str) -> anyhow::Result<Probed> {
    let mut probed = Probed::Played(ToolChoice::Required);
    for game in Name::ALL {
        let Probed::Played(from) = probed else {
            break;
        };
        probed = game.check_plays(model, id, from).await?;
    }
    Ok(probed)
}

/// What asking the model `spec` describes to play finds, with its key
/// looked up in `keys`, as `social-deduction models --check <id>` asks it.
///
/// # Errors
///
/// When its key is not set, its provider does not serve it, or it could
/// not be asked.
pub async fn check(spec: &Spec, keys: &dyn Keys) -> anyhow::Result<Check> {
    let provider = spec.provider(keys)?;
    provider.listed(&spec.id).await?;
    probe(&provider, &spec.id).await
}

/// What asking `model` to play as the model `id` finds, as [`plays`]
/// asks it.
///
/// # Errors
///
/// When the model could not be asked.
pub async fn probe(model: &dyn Model, id: &str) -> anyhow::Result<Check> {
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
    /// Asked to play each game, it answered every request well, under the
    /// tool choice every game allows.
    Played(ToolChoice),
    /// Asked to play each game, it did not answer one request well.
    DidNotPlay,
}

/// Each of `ids`, in order, with its mark: those in `check`, or every one
/// of them when `check` is empty, are asked by `model` to play each game in
/// [`Name::ALL`], one model at a time, since a local server loads each
/// model it is asked for, and the rest are unmarked. Each game's probe
/// starts from the tool choice the games before it settled. Why a model that did not play failed
/// goes to `reasons`, as [`reason`] writes it. A model that could not be
/// asked stops the listing with the error, since nothing it would say
/// about the rest would be true.
pub async fn checked(
    model: &dyn Model,
    ids: Vec<String>,
    check: &[String],
    reasons: &mut impl Write,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned::{self, refusal};
    use crate::model::fake::{FakeModel, choosing_first};
    use crate::model::spec::Environment;
    use crate::model::spec::tests::{LOCAL, qwen};
    use crate::model::{Provider, REQUEST_TIMEOUT};

    /// A model that plays as `id` and refuses as anything else, over two
    /// lines.
    fn playing_only_as(id: &'static str) -> FakeModel {
        FakeModel::answering(move |request| {
            if request.model == id {
                choosing_first(request)
            } else {
                Err(anyhow::anyhow!("{}\nin two lines", refusal(&request.model)))
            }
        })
    }

    #[tokio::test]
    async fn every_model_listed_is_checked_unless_ids_are_given_and_then_only_those() {
        let ids = ["gpt-0", "llama", "qwen2.5-7b-instruct"].map(String::from);
        let model = playing_only_as("qwen2.5-7b-instruct");
        let mut reasons = Vec::new();
        let marked = checked(&model, ids.to_vec(), &[], &mut reasons)
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
        // A model that plays Werewolf by night is asked again as by day,
        // then as each of the four Avalon asks after.
        let asked: Vec<String> = model.requests().into_iter().map(|r| r.model).collect();
        assert_eq!(
            asked,
            [&["gpt-0", "llama"][..], &["qwen2.5-7b-instruct"; 6][..]].concat()
        );
        // Each reason under a line naming its model as the listing marks
        // it, the error's causes one per indented line, a cause of several
        // lines indented on each.
        assert_eq!(
            String::from_utf8(reasons).unwrap(),
            "! gpt-0\n  gpt-0 did not play Werewolf\n  did not answer a night's request\n  \
             gpt-0 does not support tools\n  in two lines\n\
             ! llama\n  llama did not play Werewolf\n  did not answer a night's request\n  \
             llama does not support tools\n  in two lines\n"
        );
        // Only the ids given, in the listing's order.
        let model = playing_only_as("qwen2.5-7b-instruct");
        let check = ["qwen2.5-7b-instruct", "gpt-0"].map(String::from);
        let mut reasons = Vec::new();
        let marked = checked(&model, ids.to_vec(), &check, &mut reasons)
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
        assert_eq!(
            asked,
            [&["gpt-0"][..], &["qwen2.5-7b-instruct"; 6][..]].concat()
        );
        assert!(String::from_utf8(reasons).unwrap().starts_with("! gpt-0\n"));
    }

    #[tokio::test]
    async fn a_model_made_to_call_a_tool_in_one_game_but_not_the_other_is_marked_as_not_made_to() {
        let base_url = canned::refusing_required_offering("propose", &["llama"]);
        let provider = Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap();
        let mut reasons = Vec::new();
        let marked = checked(&provider, vec!["llama".to_string()], &[], &mut reasons)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [("llama".to_string(), Some(Mark::Played(ToolChoice::Auto)))]
        );
        assert!(reasons.is_empty());
    }

    #[tokio::test]
    async fn a_model_that_plays_only_when_not_made_to_call_a_tool_is_marked_so() {
        let base_url = canned::refusing_required(&["sonnet"]);
        let provider = Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap();
        let mut reasons = Vec::new();
        let marked = checked(&provider, vec!["sonnet".to_string()], &[], &mut reasons)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [("sonnet".to_string(), Some(Mark::Played(ToolChoice::Auto)))]
        );
        assert!(reasons.is_empty());
    }

    /// A model asked through a provider, remembering the tool choice each
    /// request to it was made under.
    struct Watched {
        provider: Provider,
        asked: std::sync::Mutex<Vec<ToolChoice>>,
    }

    #[async_trait::async_trait]
    impl Model for Watched {
        async fn complete(
            &self,
            request: &crate::model::Request,
        ) -> anyhow::Result<crate::model::Response> {
            self.asked.lock().unwrap().push(request.tool_choice);
            self.provider.complete(request).await
        }
    }

    #[tokio::test]
    async fn a_model_that_refuses_to_be_made_to_call_a_tool_is_not_made_to_again_in_the_next_game()
    {
        let base_url = canned::refusing_required(&["sonnet"]);
        let model = Watched {
            provider: Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap(),
            asked: Default::default(),
        };
        let mut reasons = Vec::new();
        let marked = checked(&model, vec!["sonnet".to_string()], &[], &mut reasons)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [("sonnet".to_string(), Some(Mark::Played(ToolChoice::Auto)))]
        );
        // Refused once by night in Werewolf, then asked to call a tool but
        // not made to: by night again, by day, and as each of Avalon's four.
        let asked = model.asked.into_inner().unwrap();
        assert_eq!(
            asked,
            [&[ToolChoice::Required][..], &[ToolChoice::Auto; 6][..]].concat()
        );
    }

    #[tokio::test]
    async fn a_model_that_could_not_be_asked_stops_the_listing_with_the_error() {
        // A provider answering anything but the model's answer says nothing
        // about this model or the rest.
        let (base_url, _served) = canned::answering("429 Too Many Requests", "{}");
        let provider = Provider::new(base_url.clone(), None, REQUEST_TIMEOUT).unwrap();
        let ids = ["gpt-0", "llama"].map(String::from);
        let mut reasons = Vec::new();
        let error = checked(&provider, ids.to_vec(), &[], &mut reasons)
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
        let spec = qwen(&canned::playing(&["qwen2.5-7b-instruct"]), "");
        assert_eq!(check(&spec, &Environment).await.unwrap(), Check::Plays);
        let spec = qwen(&canned::refusing_required(&["qwen2.5-7b-instruct"]), "");
        assert_eq!(
            check(&spec, &Environment).await.unwrap(),
            Check::PlaysUnforced
        );
        let spec = qwen(&canned::playing_only(&["qwen2.5-7b-instruct"], &[]), "");
        let checked = check(&spec, &Environment).await.unwrap();
        let did_not_play = |checked: &Check| {
            matches!(checked, Check::DidNotPlay(reason)
                if reason.starts_with("qwen2.5-7b-instruct did not play Werewolf"))
        };
        assert!(did_not_play(&checked), "{checked:?}");
    }

    #[tokio::test]
    async fn the_cause_of_a_model_not_playing_is_what_the_provider_answered_the_request() {
        let model = "qwen2.5-7b-instruct";
        let spec = qwen(&canned::refusing_days(&[model]), "");
        let checked = check(&spec, &Environment).await.unwrap();
        let cause = checked.cause().unwrap();
        assert!(cause.starts_with("POST "), "{cause}");
        assert!(
            cause.ends_with(&format!("400 Bad Request: {}", canned::day_refusal(model))),
            "{cause}"
        );
        assert_eq!(Check::Plays.cause(), None);
    }

    #[tokio::test]
    async fn a_model_not_served_or_without_its_key_is_an_error() {
        let spec = qwen(&canned::serving(&["llama"]), "");
        let error = check(&spec, &Environment).await.unwrap_err();
        assert!(
            format!("{error}").contains("does not serve a model with the id qwen2.5-7b-instruct")
        );
        let spec = qwen(LOCAL, "api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"");
        let error = check(&spec, &Environment).await.unwrap_err();
        assert_eq!(
            format!("{error}"),
            "SOCIAL_DEDUCTION_NO_SUCH_KEY is not set"
        );
    }
}
