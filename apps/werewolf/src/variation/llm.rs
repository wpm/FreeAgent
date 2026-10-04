//! The player that is a language model.
//!
//! Everything the player hears goes into one conversation with the model,
//! and everything the player says or decides comes out of it. A turn by
//! day asks the model to speak and lets it nominate with a tool; a kill
//! by night tells it to nominate and asks again if it does not.
//!
//! The tool is never forced. Current Claude models reject a forced tool
//! choice, and an instruction in the prompt plus a second chance does
//! the same job everywhere.

use crate::game::{Message, Phase, PlayerId, Role, Team};
use crate::config::{LlmConfig, Prompts};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::sleep;

/// The one tool the model has: naming a living player.
pub const NOMINATE: &str = "nominate";

/// A player whose every word and choice comes from a model.
pub struct Llm {
    config: LlmConfig,
    api_key: SecretString,
    prompts: Prompts,
    client: Client,
    history: Vec<ChatMessage>,
}

impl Llm {
    /// A player on the given model. The key is kept secret: it goes into
    /// a request header and nowhere else.
    pub fn new(config: LlmConfig, api_key: SecretString, prompts: Prompts, client: Client) -> Self {
        Llm {
            config,
            api_key,
            prompts,
            client,
            history: Vec::new(),
        }
    }

    /// What the model has been told and has said so far.
    pub fn history(&self) -> &[ChatMessage] {
        &self.history
    }

    /// Begin the conversation: the role's prompt, then who is who.
    fn introduce(
        &mut self,
        me: &PlayerId,
        role: Role,
        players: &[PlayerId],
        werewolves: &[PlayerId],
    ) {
        let mut prompt = format!(
            "{}\n\nYou are {me}. The players are {}.",
            self.prompts.for_role(role),
            players.join(", ")
        );
        if !werewolves.is_empty() {
            prompt.push_str(&format!(" The werewolves are {}.", werewolves.join(", ")));
        }
        self.history = vec![ChatMessage::System { content: prompt }];
    }

    /// Tell the model something.
    fn remember(&mut self, text: String) {
        self.history.push(ChatMessage::User { content: text });
    }

    /// Ask the model for its next turn, letting it nominate one of the
    /// candidates. Returns what it said and whom it nominated.
    async fn complete(
        &mut self,
        candidates: &[PlayerId],
    ) -> Result<(Option<String>, Option<PlayerId>)> {
        let request = ChatRequest {
            model: &self.config.model,
            messages: &self.history,
            tools: vec![nominate_tool(candidates)],
            tool_choice: json!("auto"),
            temperature: self.config.temperature,
        };
        let response = self.post(&request).await?;
        let message = response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message)
            .context("the model answered with no choices")?;
        let ChatMessage::Assistant {
            content,
            tool_calls,
        } = &message
        else {
            bail!("the model did not answer as the assistant: {message:?}");
        };
        let said = content
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(String::from);
        let calls = tool_calls.clone().unwrap_or_default();
        self.history.push(message.clone());

        let mut nominated = None;
        for call in calls {
            let outcome = match nomination(&call, candidates) {
                Ok(name) => {
                    nominated = Some(name.clone());
                    format!("{name} nominated.")
                }
                Err(error) => format!("Ignored: {error}"),
            };
            self.history.push(ChatMessage::Tool {
                content: outcome,
                tool_call_id: call.id,
            });
        }
        Ok((said, nominated))
    }

    /// Send a request to the model, trying again when the way there fails.
    async fn post(&self, request: &ChatRequest<'_>) -> Result<ChatResponse> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut last = anyhow!("no attempt was made to reach the model");
        for attempt in 0..self.config.attempts.max(1) {
            if attempt > 0 {
                sleep(Duration::from_secs(1 << attempt.min(5))).await;
            }
            let sent = self
                .client
                .post(&url)
                .bearer_auth(self.api_key.expose_secret())
                .json(request)
                .send()
                .await;
            match sent {
                Ok(response) if response.status().is_success() => {
                    return response
                        .json::<ChatResponse>()
                        .await
                        .context("the model's answer was not understood");
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    let error = anyhow!("the model answered {status}: {body}");
                    if !(status.is_server_error() || status.as_u16() == 429) {
                        return Err(error);
                    }
                    last = error;
                }
                Err(error) => last = anyhow!(error).context("could not reach the model"),
            }
        }
        Err(last)
    }
}

#[async_trait]
impl Policy for Llm {
    type Message = Message;

    async fn reply(
        &mut self,
        _from: ActorId,
        message: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>> {
        match message {
            Message::YouAre {
                role,
                players,
                werewolves,
            } => {
                self.introduce(context.id(), role, &players, &werewolves);
                Ok(None)
            }
            Message::Heard {
                from,
                said,
                nominated,
            } => {
                let mut heard = Vec::new();
                if let Some(said) = said {
                    heard.push(format!("{from} says: {said}"));
                }
                if let Some(nominated) = nominated {
                    heard.push(format!("{from} nominates {nominated}."));
                }
                self.remember(heard.join(" "));
                Ok(None)
            }
            Message::Died { player, phase } => {
                self.remember(match phase {
                    Phase::Day => format!("{player} was eliminated by the village."),
                    Phase::Night => format!("{player} was killed in the night."),
                });
                Ok(None)
            }
            Message::Turn { candidates } => {
                self.remember(format!(
                    "It is your turn. The living players are {}. Say what you want the \
                     village to hear, and you may nominate one of them to eliminate by \
                     calling {NOMINATE}.",
                    candidates.join(", ")
                ));
                let (said, nominated) = self.complete(&candidates).await?;
                Ok(Some(Message::Statement { said, nominated }))
            }
            Message::Kill { candidates } => {
                self.remember(format!(
                    "Night has fallen. The werewolves must choose a villager to kill. \
                     Call {NOMINATE} with one of {}.",
                    candidates.join(", ")
                ));
                let (_, mut nominated) = self.complete(&candidates).await?;
                if nominated.is_none() {
                    self.remember(format!(
                        "You must choose. Call {NOMINATE} with exactly one of {}.",
                        candidates.join(", ")
                    ));
                    nominated = self.complete(&candidates).await?.1;
                }
                Ok(nominated.map(Message::Choice))
            }
            Message::GameOver { winner } => {
                self.remember(format!(
                    "The game is over. The {} won.",
                    match winner {
                        Team::Werewolves => "werewolves",
                        Team::Villagers => "villagers",
                    }
                ));
                context.shutdown();
                Ok(None)
            }
            Message::Statement { .. } | Message::Choice(_) => Ok(None),
        }
    }
}

/// One message in the conversation, as the chat protocol has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatMessage {
    /// Standing instructions.
    System {
        /// The prompt.
        content: String,
    },
    /// Something the player hears.
    User {
        /// What was heard.
        content: String,
    },
    /// Something the model says, and any tools it calls.
    Assistant {
        /// What it says, if anything.
        #[serde(default)]
        content: Option<String>,
        /// The tools it calls, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    /// What came of a tool call.
    Tool {
        /// The result.
        content: String,
        /// Which call this answers.
        tool_call_id: String,
    },
}

/// A tool the model called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The call's id, quoted back in the result.
    pub id: String,
    /// Always `function`.
    #[serde(rename = "type")]
    pub kind: String,
    /// What was called and with what.
    pub function: FunctionCall,
}

/// The function part of a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// The tool's name.
    pub name: String,
    /// Its arguments, as a JSON document in a string.
    pub arguments: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    tools: Vec<Value>,
    tool_choice: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

/// The nominate tool, offering exactly the candidates.
fn nominate_tool(candidates: &[PlayerId]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": NOMINATE,
            "description": "Name one living player.",
            "parameters": {
                "type": "object",
                "properties": {
                    "player": { "type": "string", "enum": candidates }
                },
                "required": ["player"],
                "additionalProperties": false
            }
        }
    })
}

/// Whom a tool call nominates, if it is a valid nomination.
fn nomination(call: &ToolCall, candidates: &[PlayerId]) -> Result<PlayerId> {
    ensure!(
        call.function.name == NOMINATE,
        "no tool named {}",
        call.function.name
    );
    #[derive(Deserialize)]
    struct Arguments {
        player: PlayerId,
    }
    let Arguments { player } = serde_json::from_str(&call.function.arguments)
        .with_context(|| format!("bad arguments {:?}", call.function.arguments))?;
    ensure!(
        candidates.contains(&player),
        "{player} is not among {}",
        candidates.join(", ")
    );
    Ok(player)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Timing;
    use crate::game::Role;
    use free_agent::{Ending, Reply, episode};
    use std::collections::HashSet;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn names(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn the_nominate_tool_offers_exactly_the_candidates() {
        let tool = nominate_tool(&names(&["Ann", "Bob"]));

        assert_eq!(tool["function"]["name"], NOMINATE);
        assert_eq!(
            tool["function"]["parameters"]["properties"]["player"]["enum"],
            json!(["Ann", "Bob"])
        );
    }

    #[test]
    fn a_nomination_must_name_a_candidate_with_the_right_tool() {
        let candidates = names(&["Ann", "Bob"]);

        let bob = nomination(&call("1", NOMINATE, r#"{"player":"Bob"}"#), &candidates);
        assert_eq!(bob.unwrap(), "Bob");

        let stranger = nomination(&call("2", NOMINATE, r#"{"player":"Zed"}"#), &candidates);
        assert!(stranger.is_err());

        let other_tool = nomination(&call("3", "shout", r#"{"player":"Ann"}"#), &candidates);
        assert!(other_tool.is_err());

        let garbage = nomination(&call("4", NOMINATE, "Bob"), &candidates);
        assert!(garbage.is_err());
    }

    #[test]
    fn an_assistant_message_with_tool_calls_parses_as_the_protocol_sends_it() {
        let json = r#"{
            "role": "assistant",
            "content": null,
            "refusal": null,
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": { "name": "nominate", "arguments": "{\"player\":\"Bob\"}" }
            }]
        }"#;

        let message: ChatMessage = serde_json::from_str(json).unwrap();

        assert_eq!(
            message,
            ChatMessage::Assistant {
                content: None,
                tool_calls: Some(vec![call("call_1", NOMINATE, r#"{"player":"Bob"}"#)]),
            }
        );
    }

    /// A model served over HTTP by `respond`, which sees each request and
    /// gives the status and body to answer with, or `None` to stop
    /// serving. Every request also goes to the test.
    async fn serve(
        mut respond: impl FnMut(&Value) -> Option<(u16, Value)> + Send + 'static,
    ) -> (String, UnboundedReceiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (requests, received) = unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    socket.read_line(&mut line).await.unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                let Some((status, response)) = respond(&request) else {
                    break;
                };
                let _ = requests.send(request);
                let body = response.to_string();
                let reason = if status == 200 { "OK" } else { "Not OK" };
                let reply = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
                socket.flush().await.unwrap();
            }
        });
        (url, received)
    }

    /// A model that answers each request with the next canned response.
    async fn fake_model(responses: Vec<(u16, Value)>) -> (String, UnboundedReceiver<Value>) {
        let mut responses = responses.into_iter();
        serve(move |_| responses.next()).await
    }

    /// A model that plays: it reads who it is from the system prompt and
    /// nominates the first candidate that is not itself, for as long as it
    /// is asked.
    async fn fake_player() -> (String, UnboundedReceiver<Value>) {
        serve(|request| {
            let system = request["messages"][0]["content"]
                .as_str()
                .unwrap_or_default();
            let me = system
                .split("You are ")
                .nth(1)
                .and_then(|rest| rest.split('.').next())
                .unwrap_or_default();
            let candidates = request["tools"][0]["function"]["parameters"]["properties"]["player"]
                ["enum"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let choice = candidates
                .iter()
                .filter_map(Value::as_str)
                .find(|name| *name != me);
            Some((200, answer(Some("Hmm."), choice)))
        })
        .await
    }

    fn ok(response: Value) -> (u16, Value) {
        (200, response)
    }

    fn answer(content: Option<&str>, nominate: Option<&str>) -> Value {
        let mut message = json!({ "role": "assistant", "content": content });
        if let Some(name) = nominate {
            message["tool_calls"] = json!([{
                "id": "call_1",
                "type": "function",
                "function": { "name": NOMINATE, "arguments": json!({ "player": name }).to_string() }
            }]);
        }
        json!({ "choices": [{ "message": message }] })
    }

    /// Puts a script of messages to the player in order and reports each
    /// answer.
    struct Driver {
        script: Vec<Message>,
        report: UnboundedSender<Option<Message>>,
    }

    #[async_trait]
    impl Policy for Driver {
        type Message = Message;

        async fn start(&mut self, context: &Context<Message>) -> Result<()> {
            let player = HashSet::from([Some("Ann".to_string())]);
            for message in self.script.drain(..) {
                let mut replies = context.request(&player, message, None).await?;
                let answer = match replies.pop() {
                    Some((_, Reply::Message(message))) => Some(message),
                    _ => None,
                };
                self.report.send(answer)?;
            }
            let over = Message::GameOver {
                winner: Team::Villagers,
            };
            context.request(&player, over, None).await?;
            context.shutdown();
            Ok(())
        }

        async fn reply(
            &mut self,
            _from: ActorId,
            _message: Message,
            _context: &Context<Message>,
        ) -> Result<Option<Message>> {
            Ok(None)
        }
    }

    /// What came of driving a model player: how the episode ended, the
    /// player's answers, and the requests the model saw.
    struct Driven {
        ending: Result<Ending>,
        answers: Vec<Option<Message>>,
        requests: Vec<Value>,
    }

    /// Run a script against a model player on a fake model.
    async fn drive(script: Vec<Message>, responses: Vec<(u16, Value)>) -> Driven {
        let (base_url, seen) = fake_model(responses).await;
        drive_at(script, base_url, 1, seen).await
    }

    /// Run a script against a model player on whatever is at `base_url`.
    async fn drive_at(
        script: Vec<Message>,
        base_url: String,
        attempts: u32,
        mut seen: UnboundedReceiver<Value>,
    ) -> Driven {
        let config = LlmConfig {
            base_url,
            model: "fake".to_string(),
            api_key_env: "KEY".to_string(),
            temperature: Some(0.5),
            attempts,
        };
        let player = Llm::new(
            config,
            SecretString::from("sk-test"),
            Prompts::default(),
            Client::new(),
        );
        let (report, mut reported) = unbounded_channel();
        let driver = Driver { script, report };
        let actors: [(ActorId, Box<dyn Policy<Message = Message> + Send>); 2] = [
            ("driver".to_string(), Box::new(driver)),
            ("Ann".to_string(), Box::new(player)),
        ];
        let ending = episode(actors, None, Duration::from_secs(10), None).await;
        let mut answers = Vec::new();
        while let Ok(answer) = reported.try_recv() {
            answers.push(answer);
        }
        let mut requests = Vec::new();
        while let Ok(request) = seen.try_recv() {
            requests.push(request);
        }
        Driven {
            ending,
            answers,
            requests,
        }
    }

    fn you_are(role: Role) -> Message {
        Message::YouAre {
            role,
            players: names(&["Ann", "Bob", "Cat"]),
            werewolves: if role == Role::Werewolf {
                names(&["Ann"])
            } else {
                vec![]
            },
        }
    }

    #[tokio::test]
    async fn on_its_turn_the_model_speaks_and_may_nominate() {
        let script = vec![
            you_are(Role::Villager),
            Message::Heard {
                from: "Bob".to_string(),
                said: Some("Cat is quiet.".to_string()),
                nominated: Some("Cat".to_string()),
            },
            Message::Turn {
                candidates: names(&["Ann", "Bob", "Cat"]),
            },
        ];
        let responses = vec![ok(answer(Some("  I suspect Bob. "), Some("Bob")))];

        let Driven {
            ending,
            answers,
            requests,
        } = drive(script, responses).await;

        ending.unwrap();

        assert_eq!(
            answers,
            vec![
                None,
                None,
                Some(Message::Statement {
                    said: Some("I suspect Bob.".to_string()),
                    nominated: Some("Bob".to_string()),
                }),
            ]
        );
        let request = &requests[0];
        assert_eq!(request["model"], "fake");
        assert_eq!(request["temperature"], 0.5);
        assert_eq!(request["tool_choice"], "auto");
        assert_eq!(
            request["tools"][0]["function"]["parameters"]["properties"]["player"]["enum"],
            json!(["Ann", "Bob", "Cat"])
        );
        let messages = request["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        let system = messages[0]["content"].as_str().unwrap();
        assert!(
            system.contains("You are a villager") && system.contains("You are Ann"),
            "{system}"
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(
            messages[1]["content"],
            "Bob says: Cat is quiet. Bob nominates Cat."
        );
        assert_eq!(messages[2]["role"], "user");
    }

    #[tokio::test]
    async fn at_night_the_model_must_nominate_and_gets_a_second_chance() {
        let script = vec![
            you_are(Role::Werewolf),
            Message::Kill {
                candidates: names(&["Bob", "Cat"]),
            },
        ];
        let responses = vec![ok(answer(None, Some("Ann"))), ok(answer(None, Some("Cat")))];

        let Driven {
            answers, requests, ..
        } = drive(script, responses).await;

        assert_eq!(answers[1], Some(Message::Choice("Cat".to_string())));
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["tool_choice"], "auto");
        let system = requests[0]["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("The werewolves are Ann"), "{system}");
        // The second request carries the rejected call and its result.
        let messages = requests[1]["messages"].as_array().unwrap();
        let tool = messages.iter().find(|m| m["role"] == "tool").unwrap();
        assert!(
            tool["content"].as_str().unwrap().starts_with("Ignored:"),
            "{tool}"
        );
        assert_eq!(tool["tool_call_id"], "call_1");
    }

    #[tokio::test]
    async fn the_key_goes_in_the_header_and_nowhere_else() {
        let script = vec![
            you_are(Role::Villager),
            Message::Turn {
                candidates: names(&["Ann", "Bob"]),
            },
        ];

        let Driven { requests, .. } = drive(script, vec![ok(answer(Some("Hello."), None))]).await;

        assert!(!requests[0].to_string().contains("sk-test"));
    }

    fn a_turn() -> Vec<Message> {
        vec![
            you_are(Role::Villager),
            Message::Turn {
                candidates: names(&["Ann", "Bob"]),
            },
        ]
    }

    #[tokio::test]
    async fn a_server_error_is_tried_again() {
        let responses = vec![
            (503, json!({ "error": "try later" })),
            ok(answer(Some("Back."), None)),
        ];
        let (base_url, seen) = fake_model(responses).await;

        let Driven {
            ending,
            answers,
            requests,
        } = drive_at(a_turn(), base_url, 2, seen).await;

        ending.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            matches!(&answers[1], Some(Message::Statement { said: Some(said), .. }) if said == "Back."),
            "{answers:?}"
        );
    }

    #[tokio::test]
    async fn a_client_error_is_not_tried_again() {
        let responses = vec![(400, json!({ "error": "bad request" }))];
        let (base_url, seen) = fake_model(responses).await;

        let Driven {
            ending, requests, ..
        } = drive_at(a_turn(), base_url, 3, seen).await;

        let error = ending.unwrap_err().to_string();
        assert!(
            error.contains("400") && error.contains("bad request"),
            "{error}"
        );
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn a_model_that_cannot_be_reached_is_an_error() {
        let (_, seen) = unbounded_channel();

        let Driven { ending, .. } =
            drive_at(a_turn(), "http://127.0.0.1:1".to_string(), 1, seen).await;

        let error = ending.unwrap_err().to_string();
        assert!(error.contains("could not reach the model"), "{error}");
    }

    #[tokio::test]
    async fn an_answer_not_from_the_assistant_is_an_error() {
        let responses = vec![ok(
            json!({ "choices": [{ "message": { "role": "user", "content": "?" } }] }),
        )];

        let Driven { ending, .. } = drive(a_turn(), responses).await;

        let error = ending.unwrap_err().to_string();
        assert!(error.contains("did not answer as the assistant"), "{error}");
    }

    #[test]
    fn the_conversation_begins_with_the_role_and_the_cast() {
        let config = LlmConfig {
            base_url: "http://nowhere".to_string(),
            model: "m".to_string(),
            api_key_env: "KEY".to_string(),
            temperature: None,
            attempts: 1,
        };
        let mut player = Llm::new(
            config,
            SecretString::from("k"),
            Prompts::default(),
            Client::new(),
        );

        player.introduce(
            &"Ann".to_string(),
            Role::Werewolf,
            &names(&["Ann", "Bob"]),
            &names(&["Ann"]),
        );

        let [ChatMessage::System { content }] = player.history() else {
            panic!("{:?}", player.history());
        };
        assert!(
            content.contains("You are Ann.") && content.contains("The werewolves are Ann."),
            "{content}"
        );
    }

    #[tokio::test]
    async fn a_whole_game_can_be_played_by_a_model() {
        use crate::game::{Config, PolicyConfig, play};

        let (base_url, _seen) = fake_player().await;
        // The variable is this test's own, so no other reader races it.
        unsafe { std::env::set_var("WEREWOLF_LLM_TEST_KEY", "sk-test") };
        let mut config = Config::random(4, 1);
        config.names = names(&["Ann", "Bob", "Cat", "Dan"]);
        config.timing = Timing {
            day_secs: 5,
            patience_secs: 5,
        };
        config.policy = PolicyConfig::Llm(LlmConfig {
            base_url,
            model: "fake".to_string(),
            api_key_env: "WEREWOLF_LLM_TEST_KEY".to_string(),
            temperature: None,
            attempts: 1,
        });

        let outcome = play(&config, 1, None).await.unwrap();

        assert!(outcome.rounds.get() <= 4, "{outcome:?}");
    }
}
