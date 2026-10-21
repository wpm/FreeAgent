//! The providers Social Deduction's tests talk to: every canned provider
//! [`free_agent_model`] has, those that play answering as Social
//! Deduction's canned player does, with the calls
//! [`canned_calls`](super::fake::canned_calls) makes: offered one tool, it
//! calls it with the first value its enumerated parameter allows, as a
//! player's `select` tool asks for a `target` from an `enum` of candidates;
//! offered several, it speaks and acts once a phase and waits after that.
//! Nothing but tests uses this module.

pub use free_agent_model::canned::{
    CANARY, CANARY_VARIABLE, LEAKED_USERINFO, NO_CREDITS, Served, answered, answering,
    assert_redacted, bound, completion, completion_of, listing, refusal, refusing,
    refusing_required_then_busy, refusing_when, reported, required_refusal, serving, serving_and,
    unreachable, wanting_a_key,
};

use super::fake::canned_calls;
use free_agent_game::report::seat_order;
use free_agent_model::canned;
use std::cmp::Ordering;
use std::sync::{Arc, Mutex};

/// A provider that plays as the canned player does, kept for the rest of
/// the process, as [`canned::playing`] plays.
pub fn playing(ids: &[&str]) -> String {
    canned::playing(canned_calls, ids)
}

/// A provider that plays as [`playing`] does, its listing reporting a
/// context window of `tokens` for each of `ids`.
pub fn playing_within(ids: &[&str], tokens: u64) -> String {
    canned::playing_within(canned_calls, ids, tokens)
}

/// A provider that plays as [`playing`] does and keeps every completion
/// request it is sent, as JSON, in order, for a test to read.
pub fn recording(ids: &[&str]) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    canned::recording(canned_calls, ids)
}

/// A provider that serves `ids` but plays only `selecting` of them, as
/// [`canned::playing_only`] does.
pub fn playing_only(ids: &[&str], selecting: &[&str]) -> String {
    canned::playing_only(canned_calls, ids, selecting)
}

/// A provider that plays each of `ids` only when not made to call a tool,
/// as [`canned::refusing_required`] does.
pub fn refusing_required(ids: &[&str]) -> String {
    canned::refusing_required(canned_calls, ids)
}

/// A provider that plays each of `ids` as [`playing`] does, but refuses a
/// request offering the tool `tool`, as [`canned::refusing_offers_of`]
/// does.
pub fn refusing_offers_of(tool: &'static str, ids: &[&str]) -> String {
    canned::refusing_offers_of(canned_calls, tool, ids)
}

/// A provider that plays each of `ids` as [`playing`] does, but refuses a
/// request offering the tool `tool` that makes the model call a tool, as
/// [`canned::refusing_required_offering`] does.
pub fn refusing_required_offering(tool: &'static str, ids: &[&str]) -> String {
    canned::refusing_required_offering(canned_calls, tool, ids)
}

/// A provider that plays each of `ids` as [`playing`] does but gives away
/// everything it is asked with, as [`canned::leaking`] does.
pub fn leaking(ids: &[&str], key: &str) -> String {
    canned::leaking(canned_calls, ids, key)
}

/// A provider that serves `ids` and plays each by night alone, kept for
/// the rest of the process: a completion request offering one tool is
/// answered as [`playing`] answers it, and one offering several, as a
/// day's does, 400 Bad Request saying what [`day_refusal`] says, as a
/// server that takes one tool at a time does.
pub fn refusing_days(ids: &[&str]) -> String {
    refusing_when(
        canned_calls,
        ids,
        |request| request["tools"].as_array().map_or(0, Vec::len) > 1,
        day_refusal,
    )
}

/// What a canned provider says when asked for a completion from `model`
/// with several tools, which it does not take.
pub fn day_refusal(model: &str) -> String {
    format!("{model} takes one tool at a time")
}

/// A provider that plays as [`playing`] does, kept for the rest of the
/// process, except that the first seat alive retracts its vote by day:
/// whenever the canned player would speak and vote, it calls `retract`
/// after.
pub fn retracting(ids: &[&str]) -> String {
    serving_and(listing(ids, None), |request| {
        answered(canned_calls(request).map(|mut calls| {
            if first_seat_voting(request, &calls) {
                calls.push(("retract".to_string(), serde_json::json!({})));
            }
            calls
        }))
    })
}

/// Whether `calls`, the canned player's answer to `request`, are `speak`
/// then `select`, from a voter who comes before every candidate `select`
/// offers in seat order. The voter is named by the story the request tells,
/// which opens with who the player is, and by day the candidates are every
/// other player alive, so this is the first seat alive, voting by day.
fn first_seat_voting(request: &serde_json::Value, calls: &[(String, serde_json::Value)]) -> bool {
    let [(speak, _), (select, _)] = calls else {
        return false;
    };
    let asked = request["messages"]
        .as_array()
        .and_then(|messages| messages.last()?["content"].as_str())
        .unwrap_or_default();
    let me = asked
        .strip_prefix("You are ")
        .and_then(|rest| rest.split([',', '.']).next())
        .unwrap_or_default()
        .to_string();
    let mut candidates = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| &tool["function"])
        .filter(|function| function["name"] == "select")
        .filter_map(|function| function["parameters"]["properties"]["target"]["enum"].as_array())
        .flatten()
        .filter_map(serde_json::Value::as_str);
    (speak.as_str(), select.as_str()) == ("speak", "select")
        && !me.is_empty()
        && candidates.all(|candidate| seat_order(&me, &candidate.to_string()) == Ordering::Less)
}

/// A quick Werewolf configuration file whose players are the model served at
/// `base_url`, a [`leaking`] provider's, asked with the key in
/// [`CANARY_VARIABLE`], and random.
pub fn leaking_werewolf(base_url: &str) -> String {
    format!(
        "game = \"werewolf\"\n[phases]\nnight_limit = \"1s\"\nday_limit = \"2s\"\nratify_limit = \"1s\"\n\
         [models.qwen]\nbase_url = \"{base_url}\"\nid = \"qwen2.5-7b-instruct\"\n\
         api_key_env = \"{CANARY_VARIABLE}\"\n\
         [strategies.plain]\npolicy = \"prompted\"\nsystem = \"Play.\"\n\
         [roles.werewolf]\nstrategy = \"random\"\n\
         [roles.villager]\nstrategy = \"plain\"\n\
         [roles.doctor]\nstrategy = \"plain\"\n\
         [roles.seer]\nstrategy = \"plain\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fake::{SAID, by_day, selecting};
    use free_agent_model::{Model, Provider, REQUEST_TIMEOUT, Response};
    use serde_json::json;

    #[tokio::test]
    async fn the_playing_provider_speaks_and_votes_by_day_and_refuses_a_request_it_cannot_play() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let answer = provider
            .complete(&by_day(&["Player 2", "Player 3"], 60, "Day 1."))
            .await
            .unwrap();
        assert_eq!(
            answer,
            Response::calling(&[
                ("speak".to_string(), json!({"text": SAID})),
                ("select".to_string(), json!({"target": "Player 2"})),
            ])
            .reporting(reported())
        );
        // A request offering a tool it cannot use is refused as bad, as a
        // server would.
        let error = provider.complete(&selecting(&[])).await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("400 Bad Request"), "{shown}");
        assert!(shown.contains("select"), "{shown}");
    }

    #[tokio::test]
    async fn the_retracting_provider_has_the_first_seat_alive_retract_its_vote_after_voting() {
        let base_url = retracting(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let first = provider
            .complete(&by_day(
                &["Player 2", "Player 3"],
                60,
                "You are Player 1, a villager.\n\nDay 1.",
            ))
            .await
            .unwrap();
        assert_eq!(
            first,
            Response::calling(&[
                ("speak".to_string(), json!({"text": SAID})),
                ("select".to_string(), json!({"target": "Player 2"})),
                ("retract".to_string(), json!({})),
            ])
            .reporting(reported())
        );
        // Any other seat plays as the playing provider does.
        let other = provider
            .complete(&by_day(
                &["Player 1", "Player 2"],
                60,
                "You are Player 3, the seer.\n\nDay 1.",
            ))
            .await
            .unwrap();
        assert_eq!(
            other,
            Response::calling(&[
                ("speak".to_string(), json!({"text": SAID})),
                ("select".to_string(), json!({"target": "Player 1"})),
            ])
            .reporting(reported())
        );
        // Seats are in the order of their numbers, and the first is the one
        // that sorts before every candidate, not only the one it votes
        // against.
        let seated = |me: &str, candidates: &[&str]| {
            by_day(
                candidates,
                60,
                &format!("You are {me}, a villager.\n\nDay 1."),
            )
        };
        let tenth = provider
            .complete(&seated("Player 2", &["Player 10", "Player 3"]))
            .await
            .unwrap();
        assert_eq!(tenth.tool_calls().len(), 3);
        for (me, candidates) in [
            ("Player 10", &["Player 2", "Player 11"][..]),
            ("Player 3", &["Player 4", "Player 1"]),
        ] {
            let other = provider.complete(&seated(me, candidates)).await.unwrap();
            assert_eq!(other.tool_calls().len(), 2, "{me}");
        }
        // So does the first seat once it has voted, and by night.
        let again = provider
            .complete(&by_day(
                &["Player 2", "Player 3"],
                60,
                "You are Player 1, a villager.\n\nDay 1.\nYou vote against Player 2.",
            ))
            .await
            .unwrap();
        assert_eq!(
            again,
            Response::tool_call("wait", &json!({"seconds": 60})).reporting(reported())
        );
        assert_eq!(
            provider.complete(&selecting(&["Player 2"])).await.unwrap(),
            Response::tool_call("select", &json!({"target": "Player 2"})).reporting(reported())
        );
    }

    #[tokio::test]
    async fn the_provider_refusing_days_selects_by_night_and_refuses_a_request_offering_several_tools()
     {
        let base_url = refusing_days(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
        assert_eq!(
            provider.complete(&selecting(&["Player 2"])).await.unwrap(),
            Response::tool_call("select", &json!({"target": "Player 2"})).reporting(reported())
        );
        let error = provider
            .complete(&by_day(&["Player 2", "Player 3"], 60, "Day 1."))
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("400 Bad Request"), "{shown}");
        assert!(
            shown.contains(&day_refusal("qwen2.5-7b-instruct")),
            "{shown}"
        );
    }
}
