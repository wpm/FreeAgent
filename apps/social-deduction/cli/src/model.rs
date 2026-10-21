//! What Social Deduction brings to [`free_agent_model`], through which it
//! reaches its models: its probe, [`plays`], which asks a model to play
//! each of its games in turn, and, for tests, its canned player and the
//! canned providers that play as it does.

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod canned;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod fake;

use crate::config::Name;
use free_agent_model::spec::Probed;
use free_agent_model::{Model, ToolChoice};

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

#[cfg(test)]
mod tests {
    use super::canned::{self, refusal};
    use super::fake::{FakeModel, choosing_first};
    use super::*;
    use free_agent_model::check::{Mark, check, checked};
    use free_agent_model::spec::Environment;
    use free_agent_model::spec::testing::qwen;
    use free_agent_model::{Provider, REQUEST_TIMEOUT, Request, Response};
    use social_deduction_gui_types::settings::Check;
    use std::sync::Mutex;

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
    async fn a_model_is_asked_to_play_werewolf_then_avalon_and_did_not_play_the_first_it_fails() {
        let ids = ["gpt-0", "qwen2.5-7b-instruct"].map(String::from);
        let model = playing_only_as("qwen2.5-7b-instruct");
        let mut reasons = Vec::new();
        let marked = checked(&model, ids.to_vec(), &[], &mut reasons, plays)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [
                ("gpt-0".to_string(), Some(Mark::DidNotPlay)),
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
            [&["gpt-0"][..], &["qwen2.5-7b-instruct"; 6][..]].concat()
        );
        assert_eq!(
            String::from_utf8(reasons).unwrap(),
            "! gpt-0\n  gpt-0 did not play Werewolf\n  did not answer a night's request\n  \
             gpt-0 does not support tools\n  in two lines\n"
        );
    }

    #[tokio::test]
    async fn a_model_made_to_call_a_tool_in_one_game_but_not_the_other_is_marked_as_not_made_to() {
        let base_url = canned::refusing_required_offering("propose", &["llama"]);
        let provider = Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap();
        let mut reasons = Vec::new();
        let marked = checked(
            &provider,
            vec!["llama".to_string()],
            &[],
            &mut reasons,
            plays,
        )
        .await
        .unwrap();
        assert_eq!(
            marked,
            [("llama".to_string(), Some(Mark::Played(ToolChoice::Auto)))]
        );
        assert!(reasons.is_empty());
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
    async fn a_model_that_refuses_to_be_made_to_call_a_tool_is_not_made_to_again_in_the_next_game()
    {
        let base_url = canned::refusing_required(&["sonnet"]);
        let model = Watched {
            provider: Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap(),
            asked: Mutex::default(),
        };
        let probed = plays(&model, "sonnet").await.unwrap();
        assert!(
            matches!(probed, Probed::Played(ToolChoice::Auto)),
            "{probed:?}"
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
    async fn the_cause_of_a_model_not_playing_is_what_the_provider_answered_the_request() {
        let model = "qwen2.5-7b-instruct";
        let spec = qwen(&canned::refusing_days(&[model]), "");
        let checked = check(&spec, &Environment, plays).await.unwrap();
        assert!(
            matches!(&checked, Check::DidNotPlay(reason)
                if reason.starts_with("qwen2.5-7b-instruct did not play Werewolf")),
            "{checked:?}"
        );
        let cause = checked.cause().unwrap();
        assert!(cause.starts_with("POST "), "{cause}");
        assert!(
            cause.ends_with(&format!("400 Bad Request: {}", canned::day_refusal(model))),
            "{cause}"
        );
    }
}
