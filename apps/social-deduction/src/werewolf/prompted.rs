//! The prompted policy in Werewolf: what a [`Player`] asks the model, and
//! how it reads the answer. The [`config`](super::config) sets each one up:
//! its system prompt, its model, the tool choice and the context window.
//!
//! By night a player is woken once, by its announcement, and has the model
//! select one of its [candidates] with one `select` call, sent to the
//! environment alone. By day it is woken by every message it receives: the
//! announcement, another player's utterance, selection or retraction, and
//! the environment's tentative decision. A day's request offers these
//! tools, of which the model calls one and may call several: `speak`,
//! which sends what it says to every other living player and not to the
//! environment; `select`, which sends its vote to the environment and every
//! other living player, so the votes are public, and which a day that is
//! not ratified offers only until the player has voted; in a ratified day,
//! `retract`, which withdraws its vote, so that it is undecided again, and
//! goes where a vote goes; and `wait`, which asks to be woken after some
//! seconds unless something wakes the player sooner. Before a game begins,
//! [`check_plays`] asks the model once as a player would by night and once
//! as by day, so a model that does not answer either request well fails at
//! launch, in the provider's words.

use super::{Message, Move, Phase, Role, View, Werewolf, awake, candidates};
use crate::model::spec::Probed;
use crate::model::{self, Response, Tool, ToolChoice};
use crate::prompted::{
    PROBE_PROMPT, SPEAK, WAIT, arguments, first_answered, probed, request, said, speak, wait,
    waited,
};
use crate::seat::{PlayerId, seats};
use anyhow::{anyhow, bail};
use humantime::format_duration;
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::num::NonZero;
use std::time::Duration;

/// A player whose decisions a language model makes.
pub type Player = crate::prompted::Player<Werewolf>;

/// What a player remembers.
pub type Memory = crate::prompted::Memory<Werewolf>;

/// Play the phase numbered `seq`, which `view` shows and which lasts
/// `limit`: by night select, and by day say, vote or wait. In a phase the
/// player sleeps through, which a wake-up from the day before can arrive
/// in if the night is announced before the wake-up is handled, or with
/// nobody to select, the model is not asked.
pub(crate) async fn play(
    player: &Player,
    seq: u64,
    view: &View,
    limit: Duration,
) -> anyhow::Result<()> {
    let me = &player.context.id;
    let candidates = candidates(me, view);
    if !awake(me, view) || candidates.is_empty() {
        return Ok(());
    }
    match view.phase {
        Phase::Night => select_by_night(player, seq, view, limit, &candidates).await,
        Phase::Day => play_by_day(player, seq, view, limit, &candidates).await,
    }
}

/// Ask the model to select one of `candidates` in the night numbered
/// `seq`, which `view` shows and which lasts `limit`, and send the
/// selection to the environment, unless the night has begun closing by
/// the time the model answers, and then it is withheld. The model's
/// failure to select, as [`selected`] judges it, is an error, and no
/// selection; so is a request that cannot be put together or a selection
/// that cannot be sent, which are the player's own doing, and they read
/// so in the error.
async fn select_by_night(
    player: &Player,
    seq: u64,
    view: &View,
    limit: Duration,
    candidates: &[PlayerId],
) -> anyhow::Result<()> {
    let voting = Voting::of(player.mind.limits.ratified(), false);
    let situation = situation(&player.context.id, view, limit, voting)?;
    let (request, shown) = player.request(&situation, night_tools(candidates));
    let response = player.complete(seq, &request).await?;
    let target = selected(&response, candidates)?;
    player.carry_out(seq, vec![Sending::Select(target)], None, shown)
}

/// Ask the model what to do in the day numbered `seq`, which `view`
/// shows and which lasts `limit`: speak, vote for one of `candidates`
/// unless it has voted in a day that is not ratified, retract its vote
/// in a ratified day, or wait, in any number and order, with one
/// response. A response that is not well formed, as
/// [`asked`] judges it, is an error, and nothing in it is carried out.
/// Otherwise the messages are sent in the order the model asked for
/// them, each utterance to every other living player and each
/// selection and retraction to them and the environment, and a wait
/// sets a reminder; unless the day has begun closing by the time the
/// model answers, and then the utterances are sent, the selections and
/// retractions are withheld, and no reminder is set.
async fn play_by_day(
    player: &Player,
    seq: u64,
    view: &View,
    limit: Duration,
    candidates: &[PlayerId],
) -> anyhow::Result<()> {
    let voting = Voting::of(player.mind.limits.ratified(), voted(player, seq));
    let tools = day_tools(candidates, limit, voting);
    let situation = situation(&player.context.id, view, limit, voting)?;
    let (request, shown) = player.request(&situation, tools);
    let response = player.complete(seq, &request).await?;
    let Asked { sends, wait } = asked(&response, candidates, limit, voting)?;
    player.carry_out(seq, sends, wait, shown)
}

/// Whether `player` has sent a selection in the phase numbered `seq`.
fn voted(player: &Player, seq: u64) -> bool {
    player.mind.acted(
        |message| matches!(message, Message::Move(Move::Select { seq: sent, .. }) if *sent == seq),
    )
}

/// Where the game stands for `me` as `view` shows it, how long the phase
/// lasts, and what is asked of it: by night, what its role does, and by
/// day, what [`day_asked`] says of a player `voting` as it may. A
/// player awake by night is a werewolf, the doctor or the seer, and is
/// shown its own role.
fn situation(
    me: &PlayerId,
    view: &View,
    limit: Duration,
    voting: Voting,
) -> anyhow::Result<String> {
    let (phase, asked) = match (&view.phase, view.roles.get(me)) {
        (Phase::Day, _) => ("day", day_asked(voting)),
        (Phase::Night, Some(Role::Werewolf)) => {
            ("night", "Choose whom the werewolves should kill.")
        }
        (Phase::Night, Some(Role::Doctor)) => ("night", "Choose whom to protect."),
        (Phase::Night, Some(Role::Seer)) => ("night", "Choose whom to ask about."),
        (Phase::Night, Some(Role::Villager)) => bail!("{me} is a villager, who sleeps by night"),
        (Phase::Night, None) => bail!("{me} is not shown its own role"),
    };
    Ok(format!(
        "It is {phase} {}, which lasts {}. {asked}",
        view.round,
        format_duration(limit)
    ))
}

/// The most a request to the model `model_id` from `me`, dealt `role` and
/// told `prompt`, takes of the context window before its story, by
/// estimate, at a table of `alive` whose day lasts `day_limit` and whose
/// vote is `ratified` or not: the system prompt, and the first day's
/// situation and tools over every other seat, which outweigh a night's,
/// and the reply.
pub(crate) fn unstoried(
    model_id: &str,
    prompt: &str,
    me: &PlayerId,
    role: Role,
    alive: &HashSet<PlayerId>,
    day_limit: Duration,
    ratified: bool,
) -> anyhow::Result<u64> {
    let view = View {
        round: NonZero::<u8>::MIN,
        phase: Phase::Day,
        roles: HashMap::from([(me.clone(), role)]),
        alive: alive.clone(),
    };
    let voting = Voting::of(ratified, false);
    let tools = day_tools(&candidates(me, &view), day_limit, voting);
    let asked = format!("\n\n{}", situation(me, &view, day_limit, voting)?);
    let request = request(model_id, ToolChoice::Required, prompt, asked, tools);
    Ok(model::estimated(&request))
}

/// The tools a player is offered by night: [`select`] over `candidates`.
fn night_tools(candidates: &[PlayerId]) -> Vec<Tool> {
    vec![select(candidates)]
}

/// How a player may vote by day.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Voting {
    /// In a ratified day: vote, again if it likes, or retract its vote.
    Ratified,
    /// In a day that is not ratified, before it has voted: vote once.
    Once,
    /// In a day that is not ratified, once it has voted: not again.
    Voted,
}

impl Voting {
    /// How a player may vote in a day that is `ratified` or not, once it
    /// has `voted` in it or not.
    fn of(ratified: bool, voted: bool) -> Self {
        match (ratified, voted) {
            (true, _) => Voting::Ratified,
            (false, false) => Voting::Once,
            (false, true) => Voting::Voted,
        }
    }
}

/// The tools a player `voting` as it may is offered by day: [`speak`],
/// [`vote`] over `candidates` unless it has voted in a day that is not
/// ratified, [`retract`] in a ratified day, and [`wait`] up to `limit`, how
/// long the day lasts.
fn day_tools(candidates: &[PlayerId], limit: Duration, voting: Voting) -> Vec<Tool> {
    let voting = match voting {
        Voting::Ratified => vec![vote(candidates, true), retract()],
        Voting::Once => vec![vote(candidates, false)],
        Voting::Voted => vec![],
    };
    std::iter::once(speak("Say something to every other living player."))
        .chain(voting)
        .chain([wait(limit)])
        .collect()
}

/// What a player `voting` as it may is asked to do by day: what
/// [`day_tools`] offers it.
fn day_asked(voting: Voting) -> &'static str {
    match voting {
        Voting::Ratified => "You may speak, vote, retract your vote, or wait.",
        Voting::Once => "You may speak, vote, or wait.",
        Voting::Voted => "You may speak or wait.",
    }
}

/// The two placeholder candidates the probe asks about.
fn placeholders() -> Vec<PlayerId> {
    seats(2).collect()
}

/// How long the probe's day lasts.
const PROBE_DAY: Duration = Duration::from_secs(60);

/// Check that `model`, asked as a player is, plays, and find out whether
/// it may be made to call a tool: two requests of a player's shape, one
/// after the other, each with a short system prompt and over two
/// placeholder candidates. The night's asks for a selection with the
/// `select` tool, and is judged as a player's night answer is by
/// `selected`; the day's is a ratified day's, whatever the game's ratify
/// limit: it says it is day and the player may speak, vote, retract its
/// vote, or wait, offers the four tools of a ratified day over a day of
/// one minute, and is judged as a player's day answer is by `asked`.
///
/// The night's request is asked under `from`. If that requires a tool call
/// and the provider refuses it as bad, it is asked again allowing one, and
/// both refusals are given if that is refused too. The first tool choice it is answered under is the
/// game's, and the day's request is asked under it. The day's request is
/// not made when the night's fails. A model that could not be asked at
/// all, because the provider was not reached or answered with anything
/// but the model's answer, is an error naming the model, with the cause
/// beneath it.
pub async fn check_plays(
    model: &dyn model::Model,
    model_id: &str,
    from: ToolChoice,
) -> anyhow::Result<Probed> {
    let candidates = placeholders();
    let by_night = async |tool_choice| {
        let night = request(
            model_id,
            tool_choice,
            PROBE_PROMPT,
            "Select one of the candidates.".to_string(),
            night_tools(&candidates),
        );
        let response = model.complete(&night).await?;
        selected(&response, &candidates).map(drop)
    };
    let by_day = async |tool_choice| {
        let situation = format!(
            "It is day, which lasts {}. {}",
            format_duration(PROBE_DAY),
            day_asked(Voting::Ratified)
        );
        let day = request(
            model_id,
            tool_choice,
            PROBE_PROMPT,
            situation,
            day_tools(&candidates, PROBE_DAY, Voting::Ratified),
        );
        let response = model.complete(&day).await?;
        asked(&response, &candidates, PROBE_DAY, Voting::Ratified).map(drop)
    };
    let (tool_choice, night) = first_answered(from, by_night).await;
    let failed = match night {
        Err(error) => Some(("night", error)),
        Ok(()) => by_day(tool_choice).await.err().map(|error| ("day", error)),
    };
    probed::<Werewolf>(
        model_id,
        tool_choice,
        failed.map(|(phase, error)| (format!("a {phase}'s request"), error)),
    )
}

/// The name of the tool a player selects with, by night and by day.
const SELECT: &str = "select";

/// The name of the tool a player retracts its vote with, by day.
const RETRACT: &str = "retract";

/// The tool a player selects with by night: `select`, taking a `target`
/// that is one of `candidates`.
fn select(candidates: &[PlayerId]) -> Tool {
    selecting(
        "Knowing everything you know, select the best one.",
        candidates,
    )
}

/// The tool a player votes with by day: `select`, taking a `target` that
/// is one of `candidates`, described as a vote that may be changed in a
/// `ratified` day, and as final in a day that is not.
fn vote(candidates: &[PlayerId], ratified: bool) -> Tool {
    let description = match ratified {
        true => "Vote for the player you want voted out. Your latest vote is the one that counts.",
        false => "Vote for the player you want voted out. Your first vote is final.",
    };
    selecting(description, candidates)
}

/// The `select` tool, described to the model by `description`, taking a
/// `target` that is one of `candidates`.
fn selecting(description: &str, candidates: &[PlayerId]) -> Tool {
    Tool::new(
        SELECT,
        description,
        json!({
            "type": "object",
            "properties": {"target": {"type": "string", "enum": candidates}},
            "required": ["target"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a player retracts its vote with by day: `retract`, taking no
/// arguments.
fn retract() -> Tool {
    Tool::new(
        RETRACT,
        "Withdraw your vote, so that you are undecided again.",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
    )
}

/// The arguments of a [`select`] call.
#[derive(Deserialize)]
struct Selection {
    target: PlayerId,
}

/// The arguments of a [`retract`] call: none.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retraction {}

/// What a day's response asks for: the messages to send, in the order the
/// model made the calls, and how long to wait, if it asked to. Of several
/// waits, the last counts.
#[derive(Debug, Default, PartialEq)]
struct Asked {
    /// What to send, in order.
    sends: Vec<Sending>,
    /// How long to wait, if asked.
    wait: Option<Duration>,
}

impl Asked {
    /// Take in `call`, one of a day's by a player `voting` as it may, if it
    /// is well formed, as [`asked`] judges it.
    fn take(
        &mut self,
        call: &model::Call,
        candidates: &[PlayerId],
        limit: Duration,
        voting: Voting,
    ) -> anyhow::Result<()> {
        match call.name.as_str() {
            SPEAK => self.sends.push(Sending::Say(said(call)?)),
            SELECT if voting != Voting::Voted => {
                self.sends.push(Sending::Select(target(call, candidates)?));
            }
            RETRACT if voting == Voting::Ratified => {
                let Retraction {} = arguments(call)?;
                self.sends.push(Sending::Retract);
            }
            WAIT => self.wait = Some(waited(call, limit)?),
            other => bail!("the model called {other}, which is not a tool it was offered"),
        }
        Ok(())
    }
}

/// What a call has the player send: by day whatever it asked for, by
/// night its one selection.
#[derive(Debug, PartialEq)]
enum Sending {
    /// Say `text` to every other living player.
    Say(String),
    /// Vote for `target`.
    Select(PlayerId),
    /// Withdraw the vote.
    Retract,
}

impl crate::prompted::Sending<Werewolf> for Sending {
    fn speaks(&self) -> bool {
        matches!(self, Sending::Say(_))
    }

    /// The message `from` sends for this in the phase numbered `seq`,
    /// numbered `n`.
    fn message(self, seq: u64, from: &PlayerId, n: u64) -> Message {
        let from = from.clone();
        match self {
            Sending::Say(text) => Message::Say { seq, from, n, text },
            Sending::Select(target) => Message::Move(Move::Select {
                seq,
                from,
                n,
                target,
            }),
            Sending::Retract => Message::Move(Move::Retract { seq, from, n }),
        }
    }
}

/// Whom `response` selects from `candidates`: the `target` of its call to
/// [`select`]. No tool call, a call to anything else, arguments that do not
/// parse, or a target who is not a candidate is an error.
fn selected(response: &Response, candidates: &[PlayerId]) -> anyhow::Result<PlayerId> {
    let call = &response
        .tool_calls()
        .first()
        .ok_or_else(|| anyhow!("the model made no tool call"))?
        .function;
    if call.name != SELECT {
        bail!("the model called {} instead of {SELECT}", call.name);
    }
    target(call, candidates)
}

/// What a day's `response` asks for, its calls taken in the order the
/// model made them, by a player `voting` as it may. In a day that is not
/// ratified, a vote after the first, in this response or an earlier one,
/// is a call to a tool not offered. The response is judged as a whole: no
/// tool call, a call to a tool [`day_tools`] does not offer the player,
/// arguments that do not parse, a target who is not one of `candidates`,
/// text that is empty once trimmed, or seconds not between 1 and `limit`
/// in whole seconds is an error, and then nothing in it is carried out.
fn asked(
    response: &Response,
    candidates: &[PlayerId],
    limit: Duration,
    mut voting: Voting,
) -> anyhow::Result<Asked> {
    let made = response.tool_calls();
    if made.is_empty() {
        bail!("the model made no tool call");
    }
    let mut asked = Asked::default();
    for call in made {
        asked.take(&call.function, candidates, limit, voting)?;
        if voting == Voting::Once && call.function.name == SELECT {
            voting = Voting::Voted;
        }
    }
    Ok(asked)
}

/// Whom `call`, a call to [`select`], selects from `candidates`. Arguments
/// that do not parse, or a target who is not a candidate, is an error.
fn target(call: &model::Call, candidates: &[PlayerId]) -> anyhow::Result<PlayerId> {
    let Selection { target } = arguments(call)?;
    if !candidates.contains(&target) {
        bail!("the model selected {target}, who is not a candidate");
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Game, game};
    use crate::model::Request;
    use crate::model::canned;
    use crate::model::fake::{FakeModel, SAID, canned_calls, choosing_first, voted_in_this_phase};
    use crate::model::spec::{Handles, ModelInPlay};
    use crate::model::window::{ContextWindow, Source};
    use crate::model::{CONTEXT_WINDOW, Provider, REQUEST_TIMEOUT};
    use crate::prompted::tests::{
        MODEL_ID, Made, PROMPTED, a_failed_call_logs_its_error, a_failure_logs_its_usage_first,
        answering, asked_of, closes, every_call_logs_its_usage,
        every_call_logs_thinking_then_thought, failed, forgotten, narrated, received, reporting,
        sent, situated, speaking, story, told, usages, waiting,
    };
    use crate::prompted::{ANSWER, Account, Mind, Shown};
    use crate::report::FORGOTTEN;
    use crate::strategy::{Prompted, Strategy};
    use crate::werewolf::Outcome;
    use crate::werewolf::config::{Configuration, players};
    use crate::werewolf::report::Story;
    use crate::werewolf::tests::{
        Played, announced_to, announcement, at_once, between_the_deal_and_the_end, closed,
        closed_before_it_ended, closing_to, dealt, mute, observed_by, observed_by_the_environment,
        one_wolf_against, retraction, run, seen, seen_by_day, selection, selections,
        selections_sent, tentative, utterance,
    };
    use crate::werewolf::{
        Actor, ENVIRONMENT, Entry, Limits, Rules, Seat, Team, to_everyone_but, to_the_environment,
        to_the_players_but,
    };
    use free_agent::ActorId;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    /// The name of the model every test's players ask.
    const MODEL: &str = "qwen";

    /// The configuration of a game of `roles` under the default rules,
    /// every player playing [`PROMPTED`], told its prompt and asking
    /// [`MODEL`], which is made to call a tool, within the default context
    /// window.
    fn configuration(roles: &HashMap<PlayerId, Role>) -> Configuration {
        let rules = Rules::default();
        let count = |role| roles.values().filter(|r| **r == role).count();
        let all = [Role::Werewolf, Role::Villager, Role::Doctor, Role::Seer];
        let model = ModelInPlay {
            base_url: "http://localhost:1234/v1".to_string(),
            id: MODEL_ID.to_string(),
            request_timeout: REQUEST_TIMEOUT,
            api_key_env: None,
            tool_choice: ToolChoice::Required,
            context_window: ContextWindow {
                tokens: CONTEXT_WINDOW,
                source: Source::Default,
            },
        };
        let strategy = Strategy::Prompted(Prompted {
            system: "You are {{ name }}.".to_string(),
            model: MODEL.to_string(),
        });
        Configuration {
            version: env!("CARGO_PKG_VERSION").to_string(),
            role_counts: all.into_iter().map(|role| (role, count(role))).collect(),
            limits: Limits {
                night_limit: rules.night_limit,
                day_limit: rules.day_limit,
                ratify_limit: rules.ratify_limit,
            },
            strategies: BTreeMap::from([(PROMPTED.to_string(), strategy)]),
            allotments: all
                .into_iter()
                .map(|role| (role, BTreeMap::from([(PROMPTED.to_string(), count(role))])))
                .collect(),
            models: BTreeMap::from([(MODEL.to_string(), model)]),
            prompts: roles.keys().map(|id| (id.clone(), told(id))).collect(),
            prompt_parts: BTreeMap::new(),
        }
    }

    /// `configuration`, its model in play changed by `change`.
    fn changed(
        mut configuration: Configuration,
        change: impl FnOnce(&mut ModelInPlay),
    ) -> Configuration {
        change(configuration.models.get_mut(MODEL).unwrap());
        configuration
    }

    /// Run a game of `roles` under `rules`, every player a model player
    /// told its prompt and asking `model`.
    async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        model: Arc<FakeModel>,
    ) -> anyhow::Result<Played> {
        let configuration = Configuration {
            limits: Werewolf::limits(&rules),
            ..configuration(&roles)
        };
        play_configured(roles, rules, configuration, model).await
    }

    /// Run a game of `roles` under `rules` and `configuration`, every
    /// player a model player asking `model`.
    async fn play_configured(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        configuration: Configuration,
        model: Arc<FakeModel>,
    ) -> anyhow::Result<Played> {
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let deal = dealt(&roles, PROMPTED);
        let handles: Handles =
            BTreeMap::from([(MODEL.to_string(), model as Arc<dyn model::Model>)]);
        let players = players(&configuration, &deal, &handles);
        let episode = game(deal, rules, configuration, winner, players, logger);
        run(episode, won, log).await
    }

    /// Run a game of `roles` under `rules`, every player a model player
    /// told its prompt and asking `model`, except those in `puppets`, who
    /// follow their own scripts.
    async fn play_with(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        model: Arc<FakeModel>,
        mut puppets: HashMap<&str, Seat>,
    ) -> anyhow::Result<Played> {
        let configuration = configuration(&roles);
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let model: Arc<dyn model::Model> = model;
        let limits = Werewolf::limits(&rules);
        let player = |id: &PlayerId| {
            puppets.remove(id.as_str()).unwrap_or_else(|| {
                Player::init(
                    id.clone(),
                    told(id),
                    MODEL_ID.to_string(),
                    ToolChoice::Required,
                    CONTEXT_WINDOW,
                    Arc::clone(&model),
                    limits.clone(),
                )
            })
        };
        let episode = game(
            dealt(&roles, PROMPTED),
            rules,
            configuration,
            winner,
            player,
            logger,
        );
        run(episode, won, log).await
    }

    /// A werewolf who kills its first candidate `after` each night begins,
    /// and says nothing by day.
    fn hunting_after(after: Duration) -> Seat {
        hunting_after_each(move |_| after)
    }

    /// A werewolf who kills its first candidate `after` the night numbered
    /// `seq` begins, and says nothing by day.
    fn hunting_after_each(after: impl Fn(u64) -> Duration + Send + 'static) -> Seat {
        Actor::puppet(Box::new(move |me, seq, view| match view.phase {
            Phase::Night => vec![(
                after(seq),
                selection(seq, me, seq, &candidates(me, view)[0]),
            )],
            Phase::Day => vec![],
        }))
    }

    /// A werewolf who kills its first candidate at once the first night and
    /// three seconds into every night after.
    fn hunting_at_once_then_after_three_seconds() -> Seat {
        hunting_after_each(|seq| Duration::from_secs(if seq == 1 { 0 } else { 3 }))
    }

    /// Whether `request` is a day's: it offers the tool to speak with.
    fn is_day(request: &Request) -> bool {
        request.tools.iter().any(|tool| tool.name == SPEAK)
    }

    /// A model answering a day's request with `by_day` of it, and a night's
    /// as the canned player does, at once.
    fn by_day(
        by_day: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> FakeModel {
        FakeModel::answering(move |request| {
            if is_day(request) {
                by_day(request)
            } else {
                choosing_first(request)
            }
        })
    }

    /// A call to `select` voting against `target`.
    fn voting(target: &str) -> Made {
        (SELECT.to_string(), json!({"target": target}))
    }

    /// A call to `retract`.
    fn retracting() -> Made {
        (RETRACT.to_string(), json!({}))
    }

    /// The answer that waits out a day of `seconds`.
    fn waiting_out(seconds: u64) -> impl Fn(&Request) -> anyhow::Result<Response> {
        move |_| Ok(Response::calling(&[waiting(seconds)]))
    }

    /// The messages `player` logged sending but its closes, each with
    /// everyone it was sent to and how many memories the model was shown,
    /// in the log's order.
    fn sent_by<'a>(
        logged: &'a [Entry],
        player: &str,
    ) -> Vec<(&'a Message, &'a HashSet<ActorId>, Option<usize>)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    from,
                    to,
                    message,
                    seen,
                    ..
                } if from == player && !closes(message) => Some((message, to, *seen)),
                _ => None,
            })
            .collect()
    }

    /// The utterances the players logged as their actions, in the log's
    /// order.
    fn utterances(logged: &[Entry]) -> Vec<&Message> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    message: message @ Message::Say { .. },
                    ..
                } => Some(message),
                _ => None,
            })
            .collect()
    }

    /// What `player` logged withholding, each with how many memories the
    /// model was shown, in the log's order.
    fn withheld_by<'a>(logged: &'a [Entry], player: &str) -> Vec<(&'a Message, Option<usize>)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Withheld {
                    from,
                    message,
                    seen,
                    ..
                } if from == player => Some((message, *seen)),
                _ => None,
            })
            .collect()
    }

    /// Short limits, so a phase that waits them out is told apart from
    /// one that does not: the votes stand after five seconds.
    fn quick() -> Rules {
        Rules {
            night_limit: Duration::from_secs(10),
            day_limit: Duration::from_secs(10),
            ratify_limit: Duration::from_secs(5),
            ..Rules::default()
        }
    }

    /// A werewolf, a doctor and two villagers. With every player selecting
    /// its first candidate, the doctor protects whomever the werewolf
    /// picks each night, the village votes a villager out each day, and
    /// the werewolves win after two days.
    fn doctored() -> HashMap<PlayerId, Role> {
        HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("doc".to_string(), Role::Doctor),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ])
    }

    /// Two werewolves and two villagers, at parity once the first night
    /// ends, so a game of them ends with nobody selecting anything.
    fn at_parity() -> HashMap<PlayerId, Role> {
        HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ])
    }

    /// The answer that selects the werewolf itself, who is no candidate.
    fn naming_itself(_: &Request) -> anyhow::Result<Response> {
        Ok(Response::tool_call(SELECT, &json!({"target": "wolf"})))
    }

    /// A response as a provider sends it, for the shapes `Response::tool_call`
    /// cannot make.
    fn answered(said: serde_json::Value) -> Response {
        serde_json::from_value(said).unwrap()
    }

    /// The answer that makes no tool call.
    fn saying_nothing(_: &Request) -> anyhow::Result<Response> {
        Ok(answered(
            json!({"choices": [{"message": {"content": "bob"}}]}),
        ))
    }

    /// The answer of a model that is down: a request that failed, and why.
    fn down(_: &Request) -> anyhow::Result<Response> {
        Err(anyhow!("connection refused")
            .context("POST http://localhost:1234/v1/chat/completions failed"))
    }

    #[tokio::test(start_paused = true)]
    async fn the_request_tells_the_prompt_the_story_and_the_situation_and_forces_the_select_tool() {
        let model = answering(choosing_first);
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, _, _) = play(roles, Rules::default(), Arc::clone(&model))
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        let requests = model.requests();
        assert_eq!(
            requests,
            [Request::offering(
                MODEL_ID,
                vec![
                    model::Message::system("You are wolf."),
                    model::Message::user(
                        "You are wolf, the only werewolf.\n\nNight 1.\n\n\
                         It is night 1, which lasts 1m. Choose whom the werewolves should kill. \
                         Answer by calling one of the tools."
                    ),
                ],
                vec![Tool::new(
                    "select",
                    "Knowing everything you know, select the best one.",
                    json!({
                        "type": "object",
                        "properties": {"target": {"type": "string", "enum": ["ann", "bob"]}},
                        "required": ["target"],
                        "additionalProperties": false,
                    }),
                )],
                ToolChoice::Required,
            )]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn allowed_a_tool_call_a_player_that_answers_in_prose_fails_and_the_game_goes_on() {
        // Every request allows a tool call. The werewolf answers in prose by
        // night, and everyone selects its first candidate by day.
        let model = answering(|request| {
            if request.messages[1].content.contains("It is night") {
                saying_nothing(request)
            } else {
                choosing_first(request)
            }
        });
        let roles = one_wolf_against(&["ann", "bob"]);
        let configuration = changed(configuration(&roles), |model| {
            model.tool_choice = ToolChoice::Auto;
        });
        let (winner, logged, _) =
            play_configured(roles, quick(), configuration, Arc::clone(&model))
                .await
                .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(
            failed(&logged),
            [("wolf", 1, "the model made no tool call")]
        );
        let requests = model.requests();
        assert!(!requests.is_empty());
        assert!(
            requests
                .iter()
                .all(|request| request.tool_choice == ToolChoice::Auto)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_good_answer_is_the_player_s_selection_for_the_phase() {
        let model = answering(|_| Ok(Response::tool_call(SELECT, &json!({"target": "bob"}))));
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, took) = play(roles, quick(), model).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(selections(&logged), [&selection(1, "wolf", 1, "bob")]);
        assert!(took < quick().night_limit, "{took:?}");
    }

    /// How many memories each of `player`'s selections says the model was
    /// shown, in the order they were made.
    fn seen_by(logged: &[Entry], player: &str) -> Vec<Option<usize>> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    from,
                    message: Message::Move(Move::Select { .. }),
                    seen,
                    ..
                } if from == player => Some(*seen),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn each_request_begins_with_the_history_of_the_one_before_up_to_its_last_day() {
        // The doctor is asked once a night and at least once a day, and
        // each request is told from everything it had heard and done by
        // then, a past day's votes giving way to its tally.
        let model = answering(choosing_first);
        play(doctored(), Rules::default(), Arc::clone(&model))
            .await
            .unwrap();
        let asked = asked_of(&model, "doc");
        assert!(asked.len() >= 4, "{asked:?}");
        for pair in asked.windows(2) {
            let before = story(&pair[0]);
            let kept = before
                .rfind("\n\nDay ")
                .map_or(before, |day| &before[..day]);
            assert!(pair[1].starts_with(kept), "{pair:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_in_a_small_window_plays_to_the_end_its_later_requests_forgetting_talk() {
        // Everyone plays as the canned player does, at length.
        let model = answering(|request| {
            let calls: Vec<Made> = canned_calls(&serde_json::to_value(request)?)?
                .into_iter()
                .map(|(name, arguments)| match name.as_str() {
                    SPEAK => speaking(&SAID.repeat(10)),
                    _ => (name, arguments),
                })
                .collect();
            Ok(Response::calling(&calls))
        });
        let roles = doctored();
        let window = 1800;
        let configuration = changed(configuration(&roles), |model| {
            model.context_window = ContextWindow {
                tokens: window,
                source: Source::File,
            };
        });
        let (winner, logged, _) =
            play_configured(roles, quick(), configuration, Arc::clone(&model))
                .await
                .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Each request fits by the estimate the player budgets with. No
        // tokenizer is at hand to check it against; a real overflow is
        // checked by hand against a real provider.
        for request in model.requests() {
            assert!(model::estimated(&request) <= window, "{request:?}");
        }
        // The doctor tells its whole story at first, and by the end of the
        // second day forgets the oldest talk, saying so.
        let asked = asked_of(&model, "doc");
        assert!(!asked[0].contains(FORGOTTEN), "{asked:?}");
        assert!(asked.last().unwrap().contains(FORGOTTEN), "{asked:?}");
        let forgot = forgotten(&logged);
        assert_eq!(forgot[0], None);
        assert!(
            forgot.iter().any(|forgot| forgot.is_some_and(|n| n > 0)),
            "{forgot:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_within_the_window_forgets_nothing() {
        let model = answering(choosing_first);
        let (_, logged, _) = play(doctored(), quick(), Arc::clone(&model)).await.unwrap();
        for asked in asked_of(&model, "doc") {
            assert!(!asked.contains(FORGOTTEN), "{asked}");
        }
        let forgot = forgotten(&logged);
        assert!(!forgot.is_empty());
        assert!(forgot.iter().all(Option::is_none), "{forgot:?}");
    }

    #[test]
    fn the_story_kept_with_the_history_is_at_every_step_a_fresh_narration_of_it() {
        let mind = Mind::new(
            "wolf".to_string(),
            told("wolf"),
            MODEL_ID.to_string(),
            ToolChoice::Required,
            CONTEXT_WINDOW,
            Arc::new(FakeModel::failing("down")),
            Limits {
                ratify_limit: Duration::from_secs(20),
                ..Werewolf::limits(&Rules::default())
            },
        );
        let pack = [("wolf", Role::Werewolf)];
        let night_2 = View {
            round: NonZero::new(2).unwrap(),
            ..seen(&["wolf", "bob"], &pack)
        };
        // A night, a day of talk and changing votes, and the next night,
        // each message remembered as it is heard or sent.
        let script = [
            received(announcement(1, seen(&["wolf", "ann", "bob", "cat"], &pack))),
            sent(selection(1, "wolf", 1, "ann")),
            received(announcement(2, seen_by_day(&["wolf", "bob", "cat"], &pack))),
            received(utterance(2, "bob", 1, "Who killed ann?")),
            sent(utterance(2, "wolf", 2, "Not me.")),
            received(selection(2, "bob", 2, "wolf")),
            received(tentative(2, 3, Some("wolf"))),
            sent(selection(2, "wolf", 3, "cat")),
            sent(selection(2, "wolf", 4, "bob")),
            received(tentative(2, 4, None)),
            received(announcement(3, night_2)),
        ];
        for (n, event) in script.iter().enumerate() {
            mind.remember(event.clone());
            let remembered = mind.remembered.lock().unwrap();
            assert_eq!(remembered.history, script[..=n]);
            let mut fresh = Story::of("wolf".to_string(), true);
            for event in &remembered.history {
                fresh.recall(event);
            }
            assert_eq!(remembered.story.lines(), fresh.lines());
        }
        assert_eq!(
            mind.story(u64::MAX),
            (
                [
                    "You are wolf, the only werewolf.",
                    "",
                    "Night 1.",
                    "You vote to kill ann.",
                    "ann dies.",
                    "",
                    "Day 1.",
                    "bob says, \"Who killed ann?\"",
                    "You say, \"Not me.\"",
                    "bob votes against wolf (werewolf).",
                    "cat does not vote.",
                    "You vote against bob.",
                    "cat dies.",
                    "",
                    "Night 2.",
                ]
                .join("\n"),
                Shown {
                    seen: script.len(),
                    forgot: None,
                }
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_by_night_is_logged_as_no_selection_and_the_day_is_played_normally() {
        // Nobody dies the night the werewolf fails to select, which waits
        // out its limit. By day everyone selects its first candidate, and
        // ann dies by day, bringing the werewolf to parity.
        for (by_night, error) in [
            (
                down as fn(&Request) -> anyhow::Result<Response>,
                "POST http://localhost:1234/v1/chat/completions failed: connection refused",
            ),
            (saying_nothing, "the model made no tool call"),
            (
                naming_itself,
                "the model selected wolf, who is not a candidate",
            ),
        ] {
            let model = answering(move |request| {
                if request.messages[1].content.contains("It is night") {
                    by_night(request)
                } else {
                    choosing_first(request)
                }
            });
            let roles = one_wolf_against(&["ann", "bob"]);
            let (winner, logged, took) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
            assert_eq!(winner, Team::Werewolves);
            // The night runs out, and the day's votes stand after five
            // seconds.
            assert_eq!(took, quick().night_limit + Duration::from_secs(5));
            assert_eq!(failed(&logged), [("wolf", 1, error)]);
            // A failure takes no number: each player closes the night as
            // its first message, and by day speaks first, so its vote is
            // its third message, the werewolf's too.
            let mut selected = selections(&logged);
            selected.sort_by_key(|message| format!("{message:?}"));
            assert_eq!(
                selected,
                [
                    &selection(2, "ann", 3, "bob"),
                    &selection(2, "bob", 3, "ann"),
                    &selection(2, "wolf", 3, "ann")
                ]
            );
            // The werewolf's story is what it was shown and what it did,
            // and nothing of the failure.
            let asked = asked_of(&model, "wolf");
            assert!(
                story(&asked[1]).starts_with(
                    "You are wolf, the only werewolf.\n\
                     \n\
                     Night 1.\n\
                     Nobody dies.\n\
                     \n\
                     Day 1."
                ),
                "{}",
                asked[1]
            );
            assert!(
                asked.iter().all(|asked| !asked.contains("fail")),
                "{asked:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_whose_model_always_fails_is_told_failure_by_failure_and_still_ends() {
        let roles = at_parity();
        let model = Arc::new(FakeModel::failing("the model is down"));
        let (winner, entries, took) = play(roles, quick(), model).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().night_limit);
        assert!(selections(&entries).is_empty());
        let mut told = narrated(entries);
        // The werewolves fail side by side, in no particular order. The
        // night takes no time because every event is stamped at the epoch,
        // while the game took its night limit by the environment's clock.
        told[9..11].sort();
        assert_eq!(
            told.join("\n"),
            "4 players:\n\
             ann, a villager, plays prompted\n\
             bob, a villager, plays prompted\n\
             wolf1, a werewolf, plays prompted\n\
             wolf2, a werewolf, plays prompted\n\
             A night waits 10s and a day 10s, and a vote stands after 5s.\n\
             qwen's context window is 4096 tokens, the default.\n\
             \n\
             Night 1.\n\
             wolf1 (werewolf) fails to select: the model is down\n\
             wolf2 (werewolf) fails to select: the model is down\n\
             Night 1 is closing.\n\
             Nobody dies.\n\
             Night 1 took 0s.\n\
             \n\
             The werewolves win after 0 days.\n\
             The game took 10s.\n\
             Survivors:\n\
             ann (villager)\n\
             bob (villager)\n\
             wolf1 (werewolf)\n\
             wolf2 (werewolf)"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn with_nobody_to_select_the_model_is_not_asked() {
        // Two werewolves know each other, so neither has anyone to select.
        let roles = HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
        ]);
        let model = answering(choosing_first);
        let (winner, logged, took) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert!(model.requests().is_empty());
        assert!(selections(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn the_log_opens_with_the_deal_then_the_configuration_then_the_first_night() {
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, _) = play(roles.clone(), Rules::default(), answering(choosing_first))
            .await
            .unwrap();
        let between = between_the_deal_and_the_end(&logged, &Rules::default(), &roles, winner);
        assert_eq!(
            logged[1],
            Entry::Configuration(Box::new(configuration(&roles)))
        );
        // Every living player is announced the night, in seat order, each
        // shown its own view.
        let everyone = ["wolf", "ann", "bob"];
        let announced = |n: u64, to: &str, known: Role| Entry::Action {
            from: ENVIRONMENT.to_string(),
            n,
            to: HashSet::from([to.to_string()]),
            message: Message::Announce {
                seq: 1,
                n,
                view: seen(&everyone, &[(to, known)]),
                limit: Rules::default().night_limit,
            },
            seen: None,
            forgot: None,
        };
        assert_eq!(between[0], announced(1, "ann", Role::Villager));
        assert_eq!(between[1], announced(2, "bob", Role::Villager));
        assert_eq!(between[2], announced(3, "wolf", Role::Werewolf));
        // The configuration is logged once.
        let configurations = between
            .iter()
            .filter(|entry| matches!(entry, Entry::Configuration(_)))
            .count();
        assert_eq!(configurations, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_of_model_players_runs_to_a_winner() {
        let roles = doctored();
        let model = answering(choosing_first);
        let (winner, logged, _) = play(roles.clone(), Rules::default(), Arc::clone(&model))
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Each announcement, as whom it went to and what it showed.
        let announced: Vec<(&PlayerId, &View)> =
            between_the_deal_and_the_end(&logged, &Rules::default(), &roles, winner)
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Action {
                        to,
                        message: Message::Announce { view, .. },
                        ..
                    } => Some((to.iter().next().unwrap(), view)),
                    _ => None,
                })
                .collect();
        let awake = announced
            .iter()
            .filter(|(to, view)| super::awake(to, view))
            .count();
        let by_day = announced
            .iter()
            .filter(|(_, view)| view.phase == Phase::Day)
            .count();
        // Every living player is announced every phase: two nights of four
        // and three, and two days of the same. Every announcement of a
        // phase the player is awake in was asked about, and by day every
        // player was asked again about what it heard, offered no vote once
        // it had voted, since the day is not ratified. Every player awake
        // selected once a phase, and spoke once a day.
        assert_eq!(announced.len(), 14);
        assert_eq!(awake, 11);
        assert_eq!(by_day, 7);
        assert!(model.requests().len() >= awake);
        assert_eq!(selections(&logged).len(), awake);
        assert_eq!(utterances(&logged).len(), by_day);
        let situations: HashSet<String> = model
            .requests()
            .iter()
            .map(|request| situated(&request.messages[1].content).to_string())
            .collect();
        assert_eq!(
            situations,
            HashSet::from([
                "It is night 1, which lasts 1m. Choose whom the werewolves should kill."
                    .to_string(),
                "It is night 1, which lasts 1m. Choose whom to protect.".to_string(),
                "It is day 1, which lasts 1m. You may speak, vote, or wait.".to_string(),
                "It is day 1, which lasts 1m. You may speak or wait.".to_string(),
                "It is night 2, which lasts 1m. Choose whom the werewolves should kill."
                    .to_string(),
                "It is night 2, which lasts 1m. Choose whom to protect.".to_string(),
                "It is day 2, which lasts 1m. You may speak, vote, or wait.".to_string(),
                "It is day 2, which lasts 1m. You may speak or wait.".to_string(),
            ])
        );
    }

    /// A werewolf against ann, bob, cat and dan, whose model by day 1 has
    /// bob say "Vote cat." and vote against cat, and everyone else wait,
    /// and plays as the canned player does otherwise: the werewolf kills
    /// ann the first night; by day only bob votes, and cat is voted out
    /// when the vote stands; the second night the werewolf kills bob and
    /// wins.
    fn only_bob_speaks_by_day_1() -> (HashMap<PlayerId, Role>, Arc<FakeModel>) {
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan"]);
        let model = Arc::new(by_day(|request| {
            let bob = request.messages[0] == model::Message::system(told("bob"));
            let calls = if bob && !voted_in_this_phase(&request.messages[1].content) {
                vec![speaking("Vote cat."), voting("cat")]
            } else {
                vec![waiting(10)]
            };
            Ok(Response::calling(&calls))
        }));
        (roles, model)
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_that_speaks_and_selects_sends_each_in_turn_to_whom_it_concerns() {
        let (roles, model) = only_bob_speaks_by_day_1();
        let (winner, logged, took) = play(roles, quick(), model).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Night 1 ends at once, day 1 five seconds after bob's vote, and
        // night 2 at once.
        assert_eq!(took, Duration::from_secs(5));
        let bob = "bob".to_string();
        let others = ["wolf", "cat", "dan"].map(String::from);
        // Bob's utterance goes to every other living player, and its vote
        // to them and the environment, in the order the model made the
        // calls, numbered in one series after its close of the night, both
        // from the one call.
        let sent = sent_by(&logged, &bob);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].0, &utterance(2, "bob", 2, "Vote cat."));
        assert_eq!(sent[0].1, &to_the_players_but(&bob, &others));
        assert_eq!(sent[1].0, &selection(2, "bob", 3, "cat"));
        assert_eq!(sent[1].1, &to_everyone_but(&bob, &others));
        assert!(sent[0].2.is_some());
        assert_eq!(sent[0].2, sent[1].2);
        // The other players observe both; the environment only the vote.
        for player in ["wolf", "cat", "dan"] {
            let observed = observed_by(&logged, player);
            assert!(observed.contains(&("bob", 2, 2)), "{player}: {observed:?}");
            assert!(observed.contains(&("bob", 3, 2)), "{player}: {observed:?}");
        }
        assert_eq!(
            observed_by_the_environment(&logged),
            [("wolf", 1, 1), ("bob", 3, 2), ("wolf", 4, 3)]
        );
        assert_eq!(
            selections_sent(&logged),
            [
                (&selection(1, "wolf", 1, "ann"), &to_the_environment()),
                (
                    &selection(2, "bob", 3, "cat"),
                    &to_everyone_but(&bob, &others)
                ),
                (&selection(3, "wolf", 4, "bob"), &to_the_environment()),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_remembers_what_it_hears_by_day_and_is_told_it_in_its_next_request() {
        let (roles, model) = only_bob_speaks_by_day_1();
        let (_, logged, _) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
        // The werewolf's second night is told from everything it heard by
        // day: what bob said, bob's vote and the decision it made.
        let asked = asked_of(&model, "wolf");
        assert_eq!(
            asked.last().unwrap(),
            "You are wolf, the only werewolf.\n\
             \n\
             Night 1.\n\
             You vote to kill ann.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             bob says, \"Vote cat.\"\n\
             bob votes against cat.\n\
             cat does not vote.\n\
             dan does not vote.\n\
             You do not vote.\n\
             cat dies.\n\
             \n\
             Night 2.\n\
             \n\
             It is night 2, which lasts 10s. Choose whom the werewolves should kill. \
             Answer by calling one of the tools."
        );
        // What it heard counts among the memories it was shown.
        assert_eq!(seen_by(&logged, "wolf"), [Some(1), Some(7)]);
        // Every player logs observing bob's utterance, bob's vote, the
        // decision and the day's closing, in the phase it was in.
        let decision = decided(&logged);
        for player in ["wolf", "cat", "dan"] {
            let by_day: Vec<(&str, u64, u64)> = observed_by(&logged, player)
                .into_iter()
                .filter(|(_, _, seq)| *seq == 2)
                .collect();
            let announcement = announced_to(&logged, player, 2);
            assert_eq!(
                by_day,
                [
                    (ENVIRONMENT, announcement, 2),
                    ("bob", 2, 2),
                    ("bob", 3, 2),
                    (ENVIRONMENT, decision, 2),
                    (ENVIRONMENT, closing_to(&logged, player, 2), 2)
                ]
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_that_votes_and_retracts_sends_both_in_turn_and_the_others_remember_it() {
        // Bob votes against cat and retracts at once, by day 1, and everyone
        // else waits. Nobody is selected after that, so the day runs out
        // with nobody voted out. Nobody votes the next day either, and the
        // werewolf wins on the third night.
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan"]);
        let model = Arc::new(by_day(|request| {
            let bob = request.messages[0] == model::Message::system(told("bob"));
            let calls = if bob && !voted_in_this_phase(&request.messages[1].content) {
                vec![voting("cat"), retracting()]
            } else {
                vec![waiting(10)]
            };
            Ok(Response::calling(&calls))
        }));
        let (winner, logged, took) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit * 2);
        let bob = "bob".to_string();
        let others = ["wolf", "cat", "dan"].map(String::from);
        // The vote and the retraction each go to the environment and every
        // other living player, in the order the model made the calls.
        let sent = sent_by(&logged, &bob);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].0, &selection(2, "bob", 2, "cat"));
        assert_eq!(sent[1].0, &retraction(2, "bob", 3));
        for (_, to, _) in &sent {
            assert_eq!(*to, &to_everyone_but(&bob, &others));
        }
        let observed: Vec<_> = observed_by_the_environment(&logged)
            .into_iter()
            .filter(|(from, _, _)| *from == "bob")
            .collect();
        assert_eq!(observed, [("bob", 2, 2), ("bob", 3, 2)]);
        // The werewolf is woken by the retraction and remembers it, and bob
        // remembers its own.
        let wolf = asked_of(&model, "wolf");
        assert!(
            wolf.iter()
                .any(|asked| asked.contains("bob withdraws their vote and is undecided again.")),
            "{wolf:?}"
        );
        let asked = asked_of(&model, "bob");
        assert!(
            asked
                .last()
                .unwrap()
                .contains("You withdraw your vote and are undecided again."),
            "{asked:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn in_a_day_that_is_not_ratified_a_player_who_voted_is_not_offered_select_again() {
        // Bob votes against cat by day 1 and asks to wait two seconds, and
        // everyone else waits. Woken by the reminder, bob is offered no
        // vote and told it may speak or wait, and its model votes against
        // dan anyway, which is a call to a tool not offered, so nothing of
        // that response is sent.
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan"]);
        let model = Arc::new(by_day(|request| {
            let bob = request.messages[0] == model::Message::system(told("bob"));
            let target = match voted_in_this_phase(&request.messages[1].content) {
                false => "cat",
                true => "dan",
            };
            let calls = match bob {
                true => vec![voting(target), waiting(2)],
                false => vec![waiting(10)],
            };
            Ok(Response::calling(&calls))
        }));
        let rules = Rules {
            ratify_limit: Duration::ZERO,
            ..quick()
        };
        let (_, logged, _) = play(roles, rules, Arc::clone(&model)).await.unwrap();
        let votes: Vec<_> = sent_by(&logged, "bob")
            .into_iter()
            .filter(|(message, _, _)| matches!(message, Message::Move(Move::Select { seq: 2, .. })))
            .map(|(message, _, _)| message)
            .collect();
        assert_eq!(votes, [&selection(2, "bob", 2, "cat")]);
        let failed = logged.iter().any(|entry| {
            matches!(entry, Entry::Failed { player, seq: 2, error }
                if player == "bob"
                    && error == "the model called select, which is not a tool it was offered")
        });
        assert!(failed, "{logged:?}");
        let bob: Vec<Request> = model
            .requests()
            .into_iter()
            .filter(|request| request.messages[0] == model::Message::system(told("bob")))
            .filter(is_day)
            .collect();
        let again = &bob[1];
        assert!(again.tools.iter().all(|tool| tool.name != SELECT));
        assert!(situated(&again.messages[1].content).ends_with("You may speak or wait."));
        // The others remember bob's first vote, and only that one.
        for other in ["wolf", "cat", "dan"] {
            let asked = asked_of(&model, other);
            assert!(
                asked
                    .iter()
                    .any(|asked| asked.contains("bob votes against cat")),
                "{other}: {asked:?}"
            );
            assert!(
                asked
                    .iter()
                    .all(|asked| !asked.contains("bob votes against dan")),
                "{other}: {asked:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn every_model_call_logs_the_usage_reported_naming_the_player_and_the_phase() {
        let model = reporting(choosing_first);
        let (_, logged, _) = play(doctored(), quick(), Arc::clone(&model)).await.unwrap();
        every_call_logs_its_usage(&model, logged);
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_that_is_not_well_formed_logs_its_usage_before_its_failure() {
        // The werewolf names itself by night, and everyone plays by day.
        let model = reporting(|request| {
            if is_day(request) {
                choosing_first(request)
            } else {
                naming_itself(request)
            }
        });
        let roles = one_wolf_against(&["ann", "bob"]);
        let (_, logged, _) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
        a_failure_logs_its_usage_first(&logged);
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_with_no_usage_or_no_response_logs_no_usage() {
        // A game at parity ends even with a model that is down.
        let roles = at_parity();
        for answer in [
            choosing_first as fn(&Request) -> anyhow::Result<Response>,
            down,
        ] {
            let model = answering(answer);
            let (_, logged, _) = play(roles.clone(), quick(), Arc::clone(&model))
                .await
                .unwrap();
            assert!(!model.requests().is_empty());
            assert!(usages(&logged).is_empty(), "{logged:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn every_model_call_logs_thinking_then_thought_then_its_usage_then_what_came_of_it() {
        let model = reporting(choosing_first);
        let (_, logged, _) = play(doctored(), quick(), Arc::clone(&model)).await.unwrap();
        every_call_logs_thinking_then_thought(&model, logged);
    }

    #[tokio::test(start_paused = true)]
    async fn a_model_call_that_fails_logs_its_thought_failed_with_the_error() {
        let model = answering(down);
        let (_, logged, _) = play(at_parity(), quick(), Arc::clone(&model))
            .await
            .unwrap();
        let error = "POST http://localhost:1234/v1/chat/completions failed: connection refused";
        a_failed_call_logs_its_error(&model, &logged, error);
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_that_never_comes_back_holds_its_phase_closing() {
        // The werewolf kills ann the first night. By day the model asked
        // never answers, so the day closes and never ends, and nobody is
        // killed with a call out: the game runs until the episode runs out
        // of patience.
        let model = Arc::new(FakeModel::answering(choosing_first).taking(Duration::MAX));
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let puppets = HashMap::from([("wolf", hunting_after(Duration::ZERO))]);
        let error = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("ran out of patience"),
            "{error:#}"
        );
    }

    /// The number of the environment's first tentative decision.
    fn decided(logged: &[Entry]) -> u64 {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::Action {
                    from,
                    n,
                    message: Message::Move(Move::Tentative { .. }),
                    ..
                } if from == ENVIRONMENT => Some(*n),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn the_situation_is_the_phase_how_long_it_lasts_and_what_the_role_does_in_it() {
        let night = |known: &[(&str, Role)]| seen(&["wolf", "doc", "seer", "ann"], known);
        let me = |name: &str| name.to_string();
        let limit = Duration::from_secs(90);
        assert_eq!(
            situation(
                &me("wolf"),
                &night(&[("wolf", Role::Werewolf)]),
                limit,
                Voting::Ratified
            )
            .unwrap(),
            "It is night 1, which lasts 1m 30s. Choose whom the werewolves should kill."
        );
        assert_eq!(
            situation(
                &me("doc"),
                &night(&[("doc", Role::Doctor)]),
                limit,
                Voting::Ratified
            )
            .unwrap(),
            "It is night 1, which lasts 1m 30s. Choose whom to protect."
        );
        assert_eq!(
            situation(
                &me("seer"),
                &night(&[("seer", Role::Seer)]),
                limit,
                Voting::Ratified
            )
            .unwrap(),
            "It is night 1, which lasts 1m 30s. Choose whom to ask about."
        );
        // By day everyone votes, whatever its role, and may retract its
        // vote in a ratified day.
        let mut day = night(&[("ann", Role::Villager)]);
        day.phase = Phase::Day;
        day.round = NonZero::new(2).unwrap();
        assert_eq!(
            situation(&me("ann"), &day, Duration::from_secs(60), Voting::Ratified).unwrap(),
            "It is day 2, which lasts 1m. You may speak, vote, retract your vote, or wait."
        );
        assert_eq!(
            situation(&me("ann"), &day, Duration::from_secs(60), Voting::Once).unwrap(),
            "It is day 2, which lasts 1m. You may speak, vote, or wait."
        );
        // A villager sleeps by night, and a player is shown its own role.
        assert!(
            situation(
                &me("ann"),
                &night(&[("ann", Role::Villager)]),
                limit,
                Voting::Ratified
            )
            .is_err()
        );
        assert!(situation(&me("ann"), &night(&[]), limit, Voting::Ratified).is_err());
    }

    #[tokio::test]
    async fn the_probe_asks_as_by_night_and_then_as_by_day_and_passes_when_the_model_plays_both() {
        let model = FakeModel::answering(choosing_first);
        let probed = check_plays(&model, MODEL_ID, ToolChoice::Required).await;
        assert!(
            matches!(probed, Ok(Probed::Played(ToolChoice::Required))),
            "{probed:?}"
        );
        assert!(not_playing(probed).is_none());
        let requests = model.requests();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert_eq!(request.model, MODEL_ID);
            assert_eq!(request.tool_choice, ToolChoice::Required);
            assert!(request.messages[1].content.ends_with(ANSWER));
            assert_eq!(request.messages.len(), 2);
            assert_eq!(request.messages[0].role, model::Role::System);
            assert_eq!(request.messages[1].role, model::Role::User);
        }
        // The night's request offers a player's one tool by night, and the
        // day's a player's four by day, over the same candidates and a day
        // of a minute, and says what a day says.
        assert_eq!(requests[0].tools, night_tools(&placeholders()));
        assert_eq!(
            requests[1].tools,
            day_tools(&placeholders(), PROBE_DAY, Voting::Ratified)
        );
        assert!(
            requests[1].messages[1]
                .content
                .ends_with(&format!("{} {ANSWER}", day_asked(Voting::Ratified)))
        );
    }

    #[tokio::test]
    async fn the_probe_accepts_a_retraction_as_an_answer_to_the_day_s_request() {
        let model = by_day(|_| Ok(Response::calling(&[retracting()])));
        let probed = check_plays(&model, MODEL_ID, ToolChoice::Required).await;
        assert!(
            matches!(probed, Ok(Probed::Played(ToolChoice::Required))),
            "{probed:?}"
        );
        assert_eq!(model.requests().len(), 2);
    }

    #[tokio::test]
    async fn the_probe_names_the_day_s_request_when_the_model_selects_by_night_but_makes_no_call_by_day()
     {
        let model = by_day(saying_nothing);
        let error = not_playing(check_plays(&model, MODEL_ID, ToolChoice::Required).await).unwrap();
        assert_eq!(
            format!("{error:#}"),
            format!(
                "{MODEL_ID} did not play Werewolf: did not answer a day's request: the model made no tool call"
            )
        );
        assert_eq!(model.requests().len(), 2);
    }

    #[tokio::test]
    async fn the_probe_names_the_night_s_request_when_that_fails_and_never_sends_the_day_s() {
        let model = FakeModel::failing("the model is down");
        let error = not_playing(check_plays(&model, MODEL_ID, ToolChoice::Required).await).unwrap();
        assert_eq!(
            format!("{error:#}"),
            format!(
                "{MODEL_ID} did not play Werewolf: did not answer a night's request: the model is down"
            )
        );
        assert_eq!(model.requests().len(), 1);
    }

    /// The error of a probe that found the model did not play, if it found
    /// so.
    fn not_playing(probed: anyhow::Result<Probed>) -> Option<anyhow::Error> {
        match probed.unwrap() {
            Probed::Played(_) => None,
            Probed::DidNotPlay(error) => Some(error),
        }
    }

    #[tokio::test]
    async fn the_probe_allows_a_tool_call_when_the_model_refuses_to_be_made_to_call_one() {
        // The night's request is refused under "required" and answered
        // under "auto", and the day's is asked under "auto", which this
        // provider would refuse under "required".
        let base_url = canned::refusing_required(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert!(matches!(
            check_plays(&provider, MODEL_ID, ToolChoice::Required)
                .await
                .unwrap(),
            Probed::Played(ToolChoice::Auto)
        ));
    }

    #[tokio::test]
    async fn the_probe_tells_a_model_that_could_not_be_asked_after_refusing_to_be_made_to_call_a_tool()
     {
        let base_url = canned::refusing_required_then_busy(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = check_plays(&provider, MODEL_ID, ToolChoice::Required)
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(&format!(
                "{MODEL_ID} could not be asked to play: POST {base_url}/chat/completions \
                 was answered 429 Too Many Requests"
            )),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn the_probe_gives_both_refusals_when_the_night_s_request_is_refused_either_way() {
        let base_url = canned::refusing(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error =
            not_playing(check_plays(&provider, MODEL_ID, ToolChoice::Required).await).unwrap();
        let shown = format!("{error:#}");
        let refused = format!(
            "POST {base_url}/chat/completions was answered 400 Bad Request: {}",
            canned::refusal(MODEL_ID)
        );
        assert_eq!(
            shown,
            format!(
                "{MODEL_ID} did not play Werewolf: did not answer a night's request: \
                 made to call a tool: {refused}\nasked to call one: {refused}"
            )
        );
    }

    #[tokio::test]
    async fn the_probe_asks_once_when_the_provider_refuses_for_another_reason_than_a_bad_request() {
        // The provider answers one request, so a second would not be
        // answered 401.
        let (base_url, _served) =
            canned::answering("401 Unauthorized", r#"{"error": {"message": "no key"}}"#);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = check_plays(&provider, MODEL_ID, ToolChoice::Required)
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("was answered 401 Unauthorized"), "{shown}");
    }

    #[tokio::test]
    async fn the_probe_tells_a_model_that_could_not_be_asked_from_one_that_did_not_play() {
        // A provider that answers anything but success or a refusal of the
        // request as bad never put the question to the model.
        let (base_url, _served) = canned::answering("429 Too Many Requests", "{}");
        let provider = Provider::new(base_url.clone(), None, REQUEST_TIMEOUT).unwrap();
        let error = check_plays(&provider, MODEL_ID, ToolChoice::Required)
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(&format!(
                "{MODEL_ID} could not be asked to play: POST {base_url}/chat/completions \
                 was answered 429 Too Many Requests"
            )),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn the_probe_judges_each_answer_as_a_player_s_is_judged() {
        let candidates = placeholders();
        // By night, as a selection; by day, as a whole.
        let elsewhere = Response::tool_call(SELECT, &json!({"target": "Player 3"}));
        let too_long = Response::calling(&[speaking("Hello."), waiting(61)]);
        let judged = [
            ("night", selected(&elsewhere, &candidates).unwrap_err()),
            (
                "day",
                super::asked(&too_long, &candidates, PROBE_DAY, Voting::Ratified).unwrap_err(),
            ),
        ];
        let models = [
            FakeModel::responding(elsewhere),
            by_day(move |_| Ok(too_long.clone())),
        ];
        for ((phase, judged), model) in judged.into_iter().zip(models) {
            let error =
                not_playing(check_plays(&model, MODEL_ID, ToolChoice::Required).await).unwrap();
            let shown = format!("{error:#}");
            assert!(
                shown.starts_with(&format!(
                    "{MODEL_ID} did not play Werewolf: did not answer a {phase}'s request"
                )),
                "{shown}"
            );
            assert!(shown.ends_with(&format!("{judged:#}")), "{shown}");
        }
    }

    #[tokio::test]
    async fn the_probe_names_the_url_and_what_the_server_said_when_a_request_is_refused() {
        // A provider refusing the night's, and one refusing the day's after
        // answering the night's.
        let refused = [
            (
                canned::refusing(&[MODEL_ID]),
                "night",
                canned::refusal(MODEL_ID),
            ),
            (
                canned::refusing_days(&[MODEL_ID]),
                "day",
                canned::day_refusal(MODEL_ID),
            ),
        ];
        for (base_url, phase, said) in refused {
            let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
            let error =
                not_playing(check_plays(&provider, MODEL_ID, ToolChoice::Required).await).unwrap();
            let shown = format!("{error:#}");
            assert!(
                shown.starts_with(&format!(
                    "{MODEL_ID} did not play Werewolf: did not answer a {phase}'s request"
                )),
                "{shown}"
            );
            assert!(
                shown.contains(&format!(
                    "POST {base_url}/chat/completions was answered 400"
                )),
                "{shown}"
            );
            assert!(shown.ends_with(&said), "{shown}");
        }
    }

    #[test]
    fn an_answer_selects_only_by_a_select_call_whose_target_is_a_candidate() {
        let candidates = ["ann".to_string(), "bob".to_string()];
        assert_eq!(
            selected(
                &Response::tool_call(SELECT, &json!({"target": "bob"})),
                &candidates
            )
            .unwrap(),
            "bob"
        );
        let wrong = [
            ("no choice", answered(json!({"choices": []}))),
            (
                "no tool call",
                answered(json!({"choices": [{"message": {"content": "bob"}}]})),
            ),
            (
                "another tool",
                Response::tool_call("vote", &json!({"target": "bob"})),
            ),
            (
                "arguments that do not parse",
                answered(json!({
                    "choices": [{"message": {"tool_calls": [{"function": {"name": "select", "arguments": "{"}}]}}]
                })),
            ),
            ("no target", Response::tool_call(SELECT, &json!({}))),
            (
                "a target that is not a name",
                Response::tool_call(SELECT, &json!({"target": 3})),
            ),
            (
                "a target who is not a candidate",
                Response::tool_call(SELECT, &json!({"target": "wolf"})),
            ),
        ];
        for (what, response) in wrong {
            assert!(selected(&response, &candidates).is_err(), "{what}");
        }
    }

    #[test]
    fn the_vote_tool_says_the_latest_vote_counts_in_a_ratified_day_and_the_first_is_final_otherwise()
     {
        let candidates = placeholders();
        assert_eq!(
            vote(&candidates, true).description,
            "Vote for the player you want voted out. Your latest vote is the one that counts."
        );
        assert_eq!(
            vote(&candidates, false).description,
            "Vote for the player you want voted out. Your first vote is final."
        );
        for ratified in [false, true] {
            let tools = day_tools(&candidates, PROBE_DAY, Voting::of(ratified, false));
            assert!(tools.contains(&vote(&candidates, ratified)), "{ratified}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_s_vote_tool_says_which_vote_counts_as_the_configuration_s_ratify_limit_has_it()
     {
        // Everyone waits by day, so the werewolf wins on the third night.
        for (ratify_limit, ratified) in [(Duration::ZERO, false), (Duration::from_secs(5), true)] {
            let model = Arc::new(by_day(waiting_out(10)));
            let rules = Rules {
                ratify_limit,
                ..quick()
            };
            let roles = one_wolf_against(&["ann", "bob", "cat"]);
            play(roles, rules, Arc::clone(&model)).await.unwrap();
            let days: Vec<Request> = model.requests().into_iter().filter(is_day).collect();
            assert!(!days.is_empty());
            for request in days {
                let select = request.tools.iter().find(|tool| tool.name == SELECT);
                let said = &select.unwrap().description;
                let expected = &vote(&placeholders(), ratified).description;
                assert_eq!(said, expected, "{ratify_limit:?}");
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn by_day_the_request_offers_the_four_tools_and_says_the_player_may_speak_vote_retract_or_wait()
     {
        // Pat, a villager, sleeps through night 1 and is woken by day 1 by
        // its announcement, asks to wait nine seconds, and is woken again by
        // that reminder, to the same request, since nobody else says
        // anything.
        let model = Arc::new(by_day(waiting_out(9)));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", mute()),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, _, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        let candidates = ["bob", "wolf"];
        let requests = model.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        assert_eq!(
            requests[0],
            Request::offering(
                MODEL_ID,
                vec![
                    model::Message::system("You are pat."),
                    model::Message::user(
                        "You are pat, a villager.\n\nNight 1.\nYou sleep.\nann dies.\n\nDay 1.\n\n\
                         It is day 1, which lasts 10s. You may speak, vote, retract your vote, or wait. \
                         Answer by calling one of the tools."
                    ),
                ],
                vec![
                    Tool::new(
                        "speak",
                        "Say something to every other living player.",
                        json!({
                            "type": "object",
                            "properties": {"text": {"type": "string", "minLength": 1}},
                            "required": ["text"],
                            "additionalProperties": false,
                        }),
                    ),
                    Tool::new(
                        "select",
                        "Vote for the player you want voted out. Your latest vote is the one that counts.",
                        json!({
                            "type": "object",
                            "properties": {"target": {"type": "string", "enum": candidates}},
                            "required": ["target"],
                            "additionalProperties": false,
                        }),
                    ),
                    Tool::new(
                        "retract",
                        "Withdraw your vote, so that you are undecided again.",
                        json!({
                            "type": "object",
                            "properties": {},
                            "additionalProperties": false,
                        }),
                    ),
                    Tool::new(
                        "wait",
                        "Say nothing now, and be woken after this many seconds unless something wakes you sooner.",
                        json!({
                            "type": "object",
                            "properties": {"seconds": {"type": "integer", "minimum": 1, "maximum": 10}},
                            "required": ["seconds"],
                            "additionalProperties": false,
                        }),
                    ),
                ],
                ToolChoice::Required,
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn messages_heard_during_a_call_lead_to_one_more_call_made_from_all_of_them() {
        // Pat takes two seconds to answer and waits the day out. Bob says
        // five things a second into the day, while pat's first call is in
        // progress: they wake pat once more, and that call is told them all.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(2)));
        // Bob is a villager, announced by day alone.
        let bob = Actor::puppet(Box::new(|me, seq, _| {
            (1..=5)
                .map(|i| {
                    let point = utterance(seq, me, i, &format!("Point {i}."));
                    (Duration::from_secs(1), point)
                })
                .collect()
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", bob),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        let asked = asked_of(&model, "pat");
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(!asked[0].contains("bob says"), "{}", asked[0]);
        for i in 1..=5 {
            let heard = format!("bob says, \"Point {i}.\"");
            assert!(story(&asked[1]).contains(&heard), "{}", asked[1]);
        }
        // Pat observed each of them by day.
        let heard = observed_by(&logged, "pat")
            .into_iter()
            .filter(|(from, _, seq)| *from == "bob" && *seq == 2)
            .count();
        assert_eq!(heard, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_wakes_the_player_when_it_is_up_unless_a_later_wait_or_the_end_of_the_phase_came_first()
     {
        // Pat, the seer, is woken three times by day 1. By the
        // announcement, when it asks to wait five seconds and then two, of
        // which only the second counts. Two seconds in, by that reminder,
        // when it votes against bob and asks to wait six. And at once by
        // the decision its vote brings, when it asks to wait nine. Bob is
        // voted out five seconds after the vote, and pat is announced night
        // 2 then, so the reminder from its first answer, five seconds in,
        // and the one from its second, eight seconds in, are both ignored.
        // Were either honored, pat would be asked a fourth time by day,
        // answer with no call, and fail; it never fails.
        let answers = [
            vec![waiting(5), waiting(2)],
            vec![voting("bob"), waiting(6)],
            vec![waiting(9)],
        ];
        let day_calls = AtomicUsize::new(0);
        let model = Arc::new(by_day(move |_| {
            let call = day_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Response::calling(
                &answers.get(call).cloned().unwrap_or_default(),
            ))
        }));
        // The werewolf kills ann at once the first night, and takes three
        // seconds every night after.
        let wolf = hunting_at_once_then_after_three_seconds();
        let mut roles = one_wolf_against(&["ann", "bob", "cat"]);
        roles.insert("pat".to_string(), Role::Seer);
        let puppets = HashMap::from([
            ("wolf", wolf),
            ("ann", mute()),
            ("bob", mute()),
            ("cat", mute()),
        ]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Day 1 ends seven seconds in, and night 2 three seconds later.
        assert_eq!(took, Duration::from_secs(10));
        assert_eq!(asked_of(&model, "pat").len(), 5);
        assert!(failed(&logged).is_empty(), "{logged:?}");
        // Pat's closes of night 1 and day 1 are its second and fourth
        // messages.
        assert_eq!(
            selections(&logged),
            [
                &selection(1, "wolf", 1, "ann"),
                &selection(1, "pat", 1, "ann"),
                &selection(2, "pat", 3, "bob"),
                &selection(3, "pat", 5, "cat"),
                &selection(3, "wolf", 3, "cat"),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_s_response_with_one_bad_call_is_a_failure_and_nothing_in_it_is_carried_out() {
        let model = Arc::new(by_day(|_| {
            Ok(Response::calling(&[
                speaking("I accuse bob."),
                voting("pat"),
            ]))
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", mute()),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        assert_eq!(
            failed(&logged),
            [("pat", 2, "the model selected pat, who is not a candidate")]
        );
        assert!(sent_by(&logged, "pat").is_empty(), "{logged:?}");
        assert_eq!(model.requests().len(), 1);
    }

    #[test]
    fn a_day_s_response_is_judged_as_a_whole_and_its_calls_kept_in_order() {
        let candidates = ["ann".to_string(), "bob".to_string()];
        let limit = Duration::from_secs(60);
        let judged = |calls: &[Made]| {
            super::asked(
                &Response::calling(calls),
                &candidates,
                limit,
                Voting::Ratified,
            )
        };
        assert_eq!(
            judged(&[speaking("Hi."), voting("bob"), waiting(60)]).unwrap(),
            Asked {
                sends: vec![
                    Sending::Say("Hi.".to_string()),
                    Sending::Select("bob".to_string())
                ],
                wait: Some(Duration::from_secs(60)),
            }
        );
        // The messages keep the model's order, and of several waits the
        // last counts.
        assert_eq!(
            judged(&[voting("ann"), waiting(5), speaking("Hi."), waiting(1)]).unwrap(),
            Asked {
                sends: vec![
                    Sending::Select("ann".to_string()),
                    Sending::Say("Hi.".to_string())
                ],
                wait: Some(Duration::from_secs(1)),
            }
        );
        assert_eq!(
            judged(&[speaking("Hi.")]).unwrap(),
            Asked {
                sends: vec![Sending::Say("Hi.".to_string())],
                wait: None,
            }
        );
        // A vote then a retraction leaves the player undecided, and a
        // retraction then a vote leaves it with the vote: each is sent.
        assert_eq!(
            judged(&[voting("bob"), retracting()]).unwrap(),
            Asked {
                sends: vec![Sending::Select("bob".to_string()), Sending::Retract],
                wait: None,
            }
        );
        assert_eq!(
            judged(&[retracting(), voting("ann")]).unwrap(),
            Asked {
                sends: vec![Sending::Retract, Sending::Select("ann".to_string())],
                wait: None,
            }
        );
        let wrong: [(&str, Vec<Made>, &str); 11] = [
            ("no tool call", vec![], "the model made no tool call"),
            (
                "another tool",
                vec![("vote".to_string(), json!({"target": "bob"}))],
                "the model called vote, which is not a tool it was offered",
            ),
            (
                "arguments that do not parse",
                vec![(SPEAK.to_string(), json!("Hi."))],
                "the arguments of the model's speak call",
            ),
            (
                "no text",
                vec![(SPEAK.to_string(), json!({}))],
                "speak call",
            ),
            (
                "empty text",
                vec![speaking("")],
                "the model's speak call says nothing",
            ),
            (
                "blank text",
                vec![speaking(" \n")],
                "the model's speak call says nothing",
            ),
            (
                "a target who is not a candidate",
                vec![speaking("Hi."), voting("cat")],
                "the model selected cat, who is not a candidate",
            ),
            (
                "too short a wait",
                vec![waiting(0)],
                "the model asked to wait 0s, which is not between 1s and 1m",
            ),
            (
                "too long a wait",
                vec![waiting(61)],
                "the model asked to wait 61s, which is not between 1s and 1m",
            ),
            (
                "a wait that is not a number",
                vec![(WAIT.to_string(), json!({"seconds": "soon"}))],
                "the arguments of the model's wait call",
            ),
            (
                "a retraction with arguments",
                vec![
                    voting("bob"),
                    (RETRACT.to_string(), json!({"target": "bob"})),
                ],
                "the arguments of the model's retract call",
            ),
        ];
        for (what, calls, said) in wrong {
            let error = judged(&calls).unwrap_err();
            assert!(format!("{error:#}").contains(said), "{what}: {error:#}");
        }
        // A response with no choice makes no tool call.
        let none = super::asked(
            &answered(json!({"choices": []})),
            &candidates,
            limit,
            Voting::Ratified,
        );
        assert!(none.is_err());
    }

    #[test]
    fn in_a_day_that_is_not_ratified_a_retraction_is_a_call_to_a_tool_not_offered() {
        let candidates = ["ann".to_string(), "bob".to_string()];
        let limit = Duration::from_secs(60);
        let judged = |calls: &[Made]| {
            super::asked(&Response::calling(calls), &candidates, limit, Voting::Once)
        };
        assert_eq!(
            judged(&[speaking("Hi."), voting("bob"), waiting(5)]).unwrap(),
            Asked {
                sends: vec![
                    Sending::Say("Hi.".to_string()),
                    Sending::Select("bob".to_string()),
                ],
                wait: Some(Duration::from_secs(5)),
            }
        );
        let error = judged(&[voting("bob"), retracting()]).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "the model called retract, which is not a tool it was offered"
        );
    }

    #[test]
    fn in_a_day_that_is_not_ratified_a_vote_after_the_first_is_a_call_to_a_tool_not_offered() {
        let candidates = ["ann".to_string(), "bob".to_string()];
        let limit = Duration::from_secs(60);
        let judged = |calls: &[Made], voting| {
            super::asked(&Response::calling(calls), &candidates, limit, voting)
        };
        let offered = "the model called select, which is not a tool it was offered";
        let twice = [voting("bob"), voting("ann")];
        let error = judged(&twice, Voting::Once).unwrap_err();
        assert_eq!(format!("{error:#}"), offered);
        let error = judged(&[voting("bob")], Voting::Voted).unwrap_err();
        assert_eq!(format!("{error:#}"), offered);
        assert_eq!(
            judged(&[speaking("Hi."), waiting(5)], Voting::Voted).unwrap(),
            Asked {
                sends: vec![Sending::Say("Hi.".to_string())],
                wait: Some(Duration::from_secs(5)),
            }
        );
        assert_eq!(judged(&twice, Voting::Ratified).unwrap().sends.len(), 2);
        let names: Vec<String> = day_tools(&candidates, limit, Voting::Voted)
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, [SPEAK, WAIT]);
        assert_eq!(day_asked(Voting::Voted), "You may speak or wait.");
    }

    #[test]
    fn a_day_s_tools_offer_retract_only_in_a_ratified_day() {
        let candidates = placeholders();
        let names = |ratified| -> Vec<String> {
            day_tools(&candidates, PROBE_DAY, Voting::of(ratified, false))
                .into_iter()
                .map(|tool| tool.name)
                .collect()
        };
        assert_eq!(names(true), [SPEAK, SELECT, RETRACT, WAIT]);
        assert_eq!(names(false), [SPEAK, SELECT, WAIT]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_is_offered_retract_and_told_it_may_retract_only_in_a_ratified_day() {
        // Everyone waits by day, so the werewolf wins on the third night.
        for (ratify_limit, ratified) in [(Duration::ZERO, false), (Duration::from_secs(5), true)] {
            let model = Arc::new(by_day(waiting_out(10)));
            let rules = Rules {
                ratify_limit,
                ..quick()
            };
            let roles = one_wolf_against(&["ann", "bob", "cat"]);
            play(roles, rules, Arc::clone(&model)).await.unwrap();
            let days: Vec<Request> = model.requests().into_iter().filter(is_day).collect();
            assert!(!days.is_empty());
            for request in days {
                let offered = request.tools.iter().any(|tool| tool.name == RETRACT);
                assert_eq!(offered, ratified, "{ratify_limit:?}");
                let asked = &request.messages[1].content;
                let told = format!("{} {ANSWER}", day_asked(Voting::of(ratified, false)));
                assert!(asked.ends_with(&told), "{ratify_limit:?}: {asked}");
            }
        }
    }

    #[test]
    fn arguments_sent_as_no_text_or_only_whitespace_are_no_arguments() {
        let call = |arguments: &str| model::Call {
            name: RETRACT.to_string(),
            arguments: arguments.to_string(),
        };
        for bare in ["", " \n"] {
            let Retraction {} = arguments(&call(bare)).unwrap();
        }
        let Retraction {} = arguments(&call("{}")).unwrap();
    }

    /// A game in which sam, the seer, takes three seconds to answer each
    /// request, and answers a day's with `by_day`. The werewolf kills ann
    /// three seconds into each night. Bob votes against cat as soon as day
    /// 1 opens, and the vote stands a second later, so day 1 begins to
    /// close while sam's call for its announcement is in flight. Bob says
    /// something half a second after that, while the day closes.
    async fn closed_during_a_call(
        by_day_answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> (Arc<FakeModel>, Played) {
        let model = Arc::new(by_day(by_day_answer).taking(Duration::from_secs(3)));
        // Bob is a villager, announced by day alone.
        let bob = Actor::puppet(Box::new(|me, seq, _| {
            vec![
                (Duration::ZERO, selection(seq, me, 1, "cat")),
                (
                    Duration::from_millis(1_500),
                    utterance(seq, me, 2, "Last words."),
                ),
            ]
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::from_secs(3))),
            ("ann", mute()),
            ("bob", bob),
            ("cat", mute()),
        ]);
        let mut roles = one_wolf_against(&["ann", "bob", "cat"]);
        roles.insert("sam".to_string(), Role::Seer);
        let rules = Rules {
            ratify_limit: Duration::from_secs(1),
            ..quick()
        };
        let played = play_with(roles, rules, Arc::clone(&model), puppets)
            .await
            .unwrap();
        (model, played)
    }

    /// [`closed_during_a_call`], sam's day call coming back speaking, voting
    /// and asking to wait.
    async fn answered_while_the_day_closes() -> (Arc<FakeModel>, Played) {
        closed_during_a_call(|_| {
            Ok(Response::calling(&[
                speaking(SAID),
                voting("bob"),
                waiting(2),
            ]))
        })
        .await
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_in_flight_as_the_day_closes_has_its_speech_sent_and_its_vote_withheld() {
        let (model, (winner, entries, took)) = answered_while_the_day_closes().await;
        assert_eq!(winner, Team::Werewolves);
        // Night 1 takes three seconds. Day 1 begins to close a second in,
        // and ends two seconds later, when sam's call comes back: cat is
        // voted out. Night 2 takes three seconds, and the werewolf kills
        // bob and wins.
        assert_eq!(took, Duration::from_secs(9));
        assert_eq!(
            entries
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Event(Outcome::Voted { out, .. }) => Some(out.as_deref()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [Some("cat")]
        );
        // Sam's speech is sent, after the day began to close and before it
        // ended; its vote is withheld, and sets no reminder.
        let withheld = withheld_by(&entries, "sam");
        assert_eq!(withheld.len(), 1, "{withheld:?}");
        assert_eq!(withheld[0].0, &selection(2, "sam", 0, "bob"));
        assert!(withheld[0].1.is_some());
        let sam: Vec<&Message> = sent_by(&entries, "sam")
            .into_iter()
            .map(|(message, _, _)| message)
            .collect();
        // Sam closes each phase as its second, fourth and sixth messages.
        assert_eq!(
            sam,
            [
                &selection(1, "sam", 1, "ann"),
                &utterance(2, "sam", 3, SAID),
                &selection(3, "sam", 5, "bob"),
            ]
        );
        for seq in 1..=3 {
            let living: &[&str] = match seq {
                1 => &["ann", "bob", "cat", "sam", "wolf"],
                2 => &["bob", "cat", "sam", "wolf"],
                _ => &["bob", "sam", "wolf"],
            };
            closed_before_it_ended(&entries, seq, living);
        }
        let position = |wanted: &Message| {
            entries
                .iter()
                .position(
                    |entry| matches!(entry, Entry::Action { message, .. } if message == wanted),
                )
                .unwrap()
        };
        let closing = entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    Entry::Action {
                        message: Message::Closing { seq: 2, .. },
                        ..
                    }
                )
            })
            .unwrap();
        assert!(closing < position(&utterance(2, "sam", 3, SAID)));
        // Sam is asked once by night 1, once by day 1, and once by night
        // 2: the wake-ups for bob's vote, its tentative decision and its
        // last words, which came during the day's call, start no call
        // while the day closes. Bob's last words are remembered all the
        // same.
        let asked = asked_of(&model, "sam");
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert!(
            story(&asked[2]).contains("bob says, \"Last words.\""),
            "{asked:?}"
        );
        // The console tells the day closing, then sam's speech, then whom
        // the vote put out.
        let told = narrated(entries);
        let line = |wanted: &str| {
            told.iter()
                .position(|line| line == wanted)
                .unwrap_or_else(|| panic!("{wanted}: {told:?}"))
        };
        let closing = line("Day 1 is closing.");
        let said = line(&format!("sam (seer) says, \"{SAID}\""));
        let out = line("cat dies.");
        assert!(closing < said && said < out, "{told:?}");
        assert!(
            !told.iter().any(|line| line.starts_with("sam (seer) votes")),
            "{told:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_that_fails_while_the_day_closes_still_closes_it() {
        let (_, (winner, entries, took)) = closed_during_a_call(down).await;
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(9));
        let error = "POST http://localhost:1234/v1/chat/completions failed: connection refused";
        assert_eq!(failed(&entries), [("sam", 2, error)]);
        closed_before_it_ended(&entries, 2, &["bob", "cat", "sam", "wolf"]);
        let failure = entries
            .iter()
            .position(|entry| matches!(entry, Entry::Failed { .. }))
            .unwrap();
        let closing = entries
            .iter()
            .position(|entry| {
                matches!(entry, Entry::Action { message: Message::Closed { seq: 2, from, .. }, .. }
                    if from == "sam")
            })
            .unwrap();
        assert!(failure < closing, "{entries:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_with_no_call_in_flight_closes_a_phase_at_once() {
        // Pat, a villager, sleeps through night 1, in which the werewolf
        // kills at once and wins: the night closes and the game ends at
        // once, pat asked nothing.
        let model = answering(choosing_first);
        let roles = one_wolf_against(&["ann", "pat"]);
        let puppets = HashMap::from([("wolf", hunting_after(Duration::ZERO))]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::ZERO);
        assert!(model.requests().is_empty());
        closed_before_it_ended(&logged, 1, &["ann", "pat", "wolf"]);
        // Pat's close is its only message.
        let pat: Vec<&Message> = logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action { from, message, .. } if from == "pat" => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(pat, [&closed(1, "pat", 1)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_whole_game_of_slow_model_players_withholds_no_speech() {
        // Every player takes a second and a half to answer, as the canned
        // player does, so calls are in flight whenever a phase ends.
        let model =
            Arc::new(FakeModel::answering(choosing_first).taking(Duration::from_millis(1_500)));
        let (_, logged, _) = play(doctored(), quick(), Arc::clone(&model)).await.unwrap();
        assert!(!utterances(&logged).is_empty(), "{logged:?}");
        assert!(
            !logged.iter().any(|entry| matches!(
                entry,
                Entry::Withheld {
                    message: Message::Say { .. },
                    ..
                }
            )),
            "{logged:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn by_night_a_player_is_woken_only_by_its_announcement_and_ignores_talk_from_the_day() {
        // The werewolf takes two seconds to answer and waits each day out.
        // Dan says something ten and a half seconds into day 1, which has
        // ended by then: the werewolf hears it by night 2, during its call
        // for the night, which is not asked again, observes it, and is
        // never told it.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(2)));
        // Dan is a villager, announced by day alone.
        let dan = Actor::puppet(Box::new(|me, seq, _| {
            vec![(
                Duration::from_millis(10_500),
                utterance(seq, me, 1, "Anyone there?"),
            )]
        }));
        let puppets = HashMap::from([
            ("ann", mute()),
            ("bob", mute()),
            ("cat", mute()),
            ("dan", dan),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Each night takes two seconds and each day runs out; the werewolf
        // kills ann, bob and cat in turn and is at parity with dan.
        assert_eq!(took, Duration::from_secs(26));
        let asked = asked_of(&model, "wolf");
        assert_eq!(asked.len(), 5, "{asked:?}");
        assert_eq!(
            story(&asked[3]),
            "You are wolf, the only werewolf.\n\
             \n\
             Night 1.\n\
             You vote to kill ann.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             bob does not vote.\n\
             cat does not vote.\n\
             dan does not vote.\n\
             You do not vote.\n\
             Nobody dies.\n\
             \n\
             Night 2.\n\
             You vote to kill bob.\n\
             bob dies.\n\
             \n\
             Day 2."
        );
        // The werewolf observed dan's utterance in night 2.
        assert!(
            observed_by(&logged, "wolf").contains(&("dan", 1, 3)),
            "{logged:?}"
        );
        assert!(
            asked.iter().all(|asked| !asked.contains("Anyone there?")),
            "{asked:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_announced_a_night_it_sleeps_through_is_woken_by_nothing_until_the_day() {
        // Pat, a villager, takes a tenth of a second to answer and asks to
        // wait two seconds whenever asked. The werewolf kills ann at once
        // the first night, and bob three seconds into the second. By day 1
        // bob votes against cat at once, whose vote stands a second later,
        // and says two things just before the day ends and one more during
        // night 2, which bob does not know has begun, and pat ignores. By
        // day 2 dan votes the werewolf out.
        let model = Arc::new(by_day(waiting_out(2)).taking(Duration::from_millis(100)));
        let wolf = hunting_at_once_then_after_three_seconds();
        let bob = Actor::puppet(Box::new(|me, seq, _| {
            vec![
                (Duration::ZERO, selection(seq, me, 1, "cat")),
                (
                    Duration::from_millis(950),
                    utterance(seq, me, 2, "Going quiet."),
                ),
                (Duration::from_millis(970), utterance(seq, me, 3, "Really.")),
                (
                    Duration::from_millis(2_500),
                    utterance(seq, me, 4, "Anyone there?"),
                ),
            ]
        }));
        let dan = Actor::puppet(Box::new(|me, seq, _| match seq {
            4 => at_once(vec![selection(seq, me, 1, "wolf")]),
            _ => vec![],
        }));
        let puppets = HashMap::from([
            ("wolf", wolf),
            ("ann", mute()),
            ("bob", bob),
            ("cat", mute()),
            ("dan", dan),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan", "pat"]);
        let rules = Rules {
            ratify_limit: Duration::from_secs(1),
            ..quick()
        };
        let (winner, logged, took) = play_with(roles, rules, Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Villagers);
        // Night 1 ends at once. Day 1 begins to close a second later, and
        // ends a twentieth of a second after that, when the call pat began
        // for bob's first utterance comes back. Night 2 ends three seconds
        // later, and day 2 a second after it opens.
        assert_eq!(took, Duration::from_millis(5_050));
        assert!(failed(&logged).is_empty(), "{logged:?}");
        // Pat observes each night's announcement in it, and bob's last
        // utterance in night 2, which it does not remember, and is asked
        // nothing by night. Its day reminders fall in night 2 and are
        // ignored.
        let pat = observed_by(&logged, "pat");
        for night in [1, 3] {
            let announcement = announced_to(&logged, "pat", night);
            assert!(pat.contains(&(ENVIRONMENT, announcement, night)), "{pat:?}");
        }
        assert!(pat.contains(&("bob", 4, 3)), "{pat:?}");
        let asked = asked_of(&model, "pat");
        assert!(
            asked.iter().all(|asked| !asked.contains("It is night")),
            "{asked:?}"
        );
        // By day 1 pat is asked three times: by the announcement, once for
        // bob's vote and the decision together, and by bob's first
        // utterance. That call is in progress when bob's second arrives and
        // when day 1 begins to close, and the wake-up waiting behind it
        // starts no call. By day 2 it is asked twice: by the announcement,
        // and once for dan's vote and the decision together.
        let by_day_1 = asked
            .iter()
            .filter(|asked| asked.contains("It is day 1"))
            .count();
        assert_eq!(by_day_1, 3, "{asked:?}");
        assert_eq!(asked.len(), 5, "{asked:?}");
        assert_eq!(
            story(&asked[3]),
            "You are pat, a villager.\n\
             \n\
             Night 1.\n\
             You sleep.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             bob says, \"Going quiet.\"\n\
             bob says, \"Really.\"\n\
             bob votes against cat.\n\
             cat does not vote.\n\
             dan does not vote.\n\
             You do not vote.\n\
             wolf does not vote.\n\
             cat dies.\n\
             \n\
             Night 2.\n\
             You sleep.\n\
             bob dies.\n\
             \n\
             Day 2.\n\
             dan votes against wolf."
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_ignores_messages_from_another_phase_and_takes_in_those_of_its_own() {
        // Pat, a villager, takes a second to answer and waits each day out.
        // As soon as day 1 opens, bob sends talk, a vote and a tentative
        // decision tagged with night 1, while pat's call for the
        // announcement is in progress: pat observes each and ignores it,
        // and none leaves a wake-up waiting behind the call. Three, four
        // and five seconds in, bob sends one of each tagged with the day,
        // each of which wakes pat, and pat is told them. Bob's vote against
        // the werewolf stands, and the village wins nine seconds in.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(1)));
        // Bob is a villager, announced by day alone.
        let bob = Actor::puppet(Box::new(|me, seq, _| {
            vec![
                (Duration::ZERO, utterance(seq - 1, me, 1, "Night talk.")),
                (Duration::ZERO, selection(seq - 1, me, 2, "pat")),
                (Duration::ZERO, tentative(seq - 1, 30, Some("pat"))),
                (Duration::from_secs(3), utterance(seq, me, 3, "Day talk.")),
                (Duration::from_secs(4), selection(seq, me, 4, "wolf")),
                (Duration::from_secs(5), tentative(seq, 60, None)),
            ]
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", bob),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Villagers);
        assert_eq!(took, Duration::from_secs(9));
        assert!(failed(&logged).is_empty(), "{logged:?}");
        // Pat observed all six in the day, among the environment's
        // announcement, decision and word that the day is closing. A
        // tentative decision names no sender, so bob's are observed as the
        // environment's.
        let announcement = announced_to(&logged, "pat", 2);
        let decision = decided(&logged);
        let closing = closing_to(&logged, "pat", 2);
        let by_day: Vec<(&str, u64, u64)> = observed_by(&logged, "pat")
            .into_iter()
            .filter(|(_, _, seq)| *seq == 2)
            .collect();
        assert_eq!(
            by_day,
            [
                (ENVIRONMENT, announcement, 2),
                ("bob", 1, 2),
                ("bob", 2, 2),
                (ENVIRONMENT, 30, 2),
                ("bob", 3, 2),
                ("bob", 4, 2),
                (ENVIRONMENT, decision, 2),
                (ENVIRONMENT, 60, 2),
                (ENVIRONMENT, closing, 2),
            ]
        );
        // Pat was never told the three from the night, and its second call
        // is the one bob's day talk woke, with nothing in between.
        let asked = asked_of(&model, "pat");
        assert!(asked.len() >= 4, "{asked:?}");
        for line in [
            "Night talk.",
            "bob votes against pat.",
            "pat will be voted out",
        ] {
            assert!(asked.iter().all(|asked| !asked.contains(line)), "{asked:?}");
        }
        assert!(asked[1].contains("bob says, \"Day talk.\""), "{asked:?}");
        assert_eq!(
            story(asked.last().unwrap()),
            "You are pat, a villager.\n\
             \n\
             Night 1.\n\
             You sleep.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             bob says, \"Day talk.\"\n\
             bob votes against wolf.\n\
             wolf will be voted out in 5s unless the votes stop selecting them.\n\
             Nobody is about to be voted out."
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_night_s_response_that_comes_while_the_night_closes_is_withheld_and_logged() {
        // Sam, the seer, takes three seconds to answer, and the night waits
        // two for it. The night begins to close before sam's selection
        // comes back, so nothing is sent, the selection is logged as
        // withheld, and the night ends a second later. Bob votes sam out at
        // once, and the vote stands two seconds later, while sam's call for
        // the day is in flight. The day ends when it comes back, and the
        // werewolf wins six seconds in.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(3)));
        let bob = Actor::puppet(Box::new(|me, seq, _| {
            at_once(vec![selection(seq, me, 1, "sam")])
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", bob),
        ]);
        let mut roles = one_wolf_against(&["ann", "bob"]);
        roles.insert("sam".to_string(), Role::Seer);
        let rules = Rules {
            night_limit: Duration::from_secs(2),
            ratify_limit: Duration::from_secs(2),
            ..quick()
        };
        let (winner, entries, took) = play_with(roles, rules, Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(6));
        assert!(failed(&entries).is_empty(), "{entries:?}");
        let withheld = withheld_by(&entries, "sam");
        assert_eq!(withheld.len(), 1, "{withheld:?}");
        assert_eq!(withheld[0].0, &selection(1, "sam", 0, "ann"));
        assert!(withheld[0].1.is_some());
        assert!(sent_by(&entries, "sam").is_empty(), "{entries:?}");
        let told = narrated(entries);
        assert!(
            !told
                .iter()
                .any(|line| line.starts_with("sam (seer) learns")),
            "{told:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_speak_that_is_blank_is_a_failure_and_nothing_is_sent() {
        let model = Arc::new(by_day(|_| Ok(Response::calling(&[speaking(" ")]))));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", mute()),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        assert_eq!(
            failed(&logged),
            [("pat", 2, "the model's speak call says nothing")]
        );
        assert!(utterances(&logged).is_empty(), "{logged:?}");
        assert_eq!(model.requests().len(), 1);
    }
}
