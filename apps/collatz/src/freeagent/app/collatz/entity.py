"""The Collatz :class:`~freeagent.sdk.Agent` and :class:`~freeagent.sdk.Environment`.

Collatz rehearses the platform's ack-then-counter-request shape (ADR-0008) on deterministic,
LLM-free logic. The :class:`CollatzEnvironment` seeds each agent with a starting :class:`Chain` and
owns *all* game-state judgment: agents are purely reactive. A :class:`CollatzAgent`, handed a chain,
extends it by exactly one Collatz step and sends the longer chain back to the environment as its own
request; the reply it gets is a bare :class:`~freeagent.sdk.message.Ack`, never the next chain.

The environment learns which agent a returned chain belongs to from the *subject* it arrives on, not
from the message body: each agent sends its chains to a per-agent environment subject
``{episode_root}.environment.replies.{agent}``, so the single shared :class:`Chain` type stays free
of any sender field and direction is carried entirely by the subject. When a returned chain has
reached 1
the environment stops exactly that agent with :class:`~freeagent.sdk.message.StopAgent`; when every
agent's chain is complete it publishes :class:`~freeagent.sdk.message.EpisodeComplete` and shuts
itself down.

The per-agent bookkeeping and completion judgment used here are exercised directly, without a NATS
server, by the unit tests in ``tests/``; the full over-the-wire episode is a separate integration
test.
"""

from __future__ import annotations

from freeagent.app.collatz.message import Chain, extend, is_complete
from freeagent.sdk import Agent, Environment
from freeagent.sdk.entity import AGENTS, DEFAULT_NATS_SERVER, ENVIRONMENT
from freeagent.sdk.message import Ack, Message
from nats.aio.msg import Msg

REPLIES = "replies"
"""Subject segment under the environment that agents send their extended chains back on.

Each agent replies on ``{episode_root}.environment.replies.{agent}``; the environment subscribes to
the wildcard ``{episode_root}.environment.replies.*`` and reads the agent's name off the subject's
last token. Kept distinct from the environment's own command subject so lifecycle traffic and
gameplay counter-requests don't share a subject.
"""


class CollatzAgent(Agent):
    """A reactive agent that extends any :class:`Chain` it is given by exactly one Collatz step.

    Handed a chain by the environment (as a queued in-domain message), the agent computes the next
    Collatz value, appends it, and sends the longer chain back to the environment as its *own*
    request -- the ack-then-counter-request shape: the reply to the environment's request is a bare
    :class:`~freeagent.sdk.message.Ack`, and the real result travels as a fresh request in the other
    direction. The agent makes no completion judgment of its own; it extends and returns, and the
    environment decides when the agent is finished (and stops it).

    :param episode_root: The root NATS subject for this agent's episode.
    :param name: This agent's identifier; also the last token of the environment reply subject it
        sends extended chains to.
    :param servers: NATS server URL(s) to connect to.
    """

    def __init__(
        self,
        episode_root: str,
        name: str,
        *,
        servers: str | list[str] = DEFAULT_NATS_SERVER,
    ) -> None:
        super().__init__(episode_root, name, servers=servers)
        self.name = name
        self.reply_subject = f"{episode_root}.{ENVIRONMENT}.{REPLIES}.{name}"

    async def process_message(self, message: Message) -> Message | None:
        """Extend an incoming :class:`Chain` and send the result back to the environment.

        Non-:class:`Chain` messages are ignored (acknowledged with the run loop's default
        :class:`~freeagent.sdk.message.Ack`). For a :class:`Chain`, the extended chain is sent to
        the environment's per-agent reply subject as a new request; the reply to *this* message
        stays a
        bare :class:`~freeagent.sdk.message.Ack` (returned as ``None``), keeping the work off the
        reply per ADR-0008.

        :param message: The message pulled from the agent's queue.
        :return: Always ``None``: the run loop then replies with a bare
            :class:`~freeagent.sdk.message.Ack`, and the extended chain has already gone out as a
            separate counter-request.
        """
        if isinstance(message, Chain):
            await self.request(self.reply_subject, extend(message))
        return None


class CollatzEnvironment(Environment):
    """Drives a Collatz episode and owns every game-state judgment in it.

    Seeds each agent with a starting :class:`Chain`, receives each agent's extended chain on that
    agent's reply subject, and decides -- as the sole judge -- when an agent has finished (its chain
    reached 1) and when the whole episode is done (every agent finished). A finished agent is
    stopped individually with :class:`~freeagent.sdk.message.StopAgent` so the others keep running;
    when the last agent finishes the environment publishes
    :class:`~freeagent.sdk.message.EpisodeComplete` and stops itself, which broadcasts the
    episode-wide :class:`~freeagent.sdk.message.StopEntity` teardown.

    :param episode_root: The root NATS subject for this episode.
    :param starts: The starting number for each agent, keyed by agent name. Its keys are the
        episode's agents; each value seeds that agent's chain.
    :param servers: NATS server URL(s) to connect to.
    :param timeout: Seconds to wait for an agent to acknowledge a broadcast or targeted command, or
        ``None`` to wait indefinitely; passed through to :class:`~freeagent.sdk.Environment`.

    :ivar chains: Each agent's latest known chain, keyed by agent name -- the environment's record
        of game state, updated every time an agent returns an extended chain.
    :ivar finished: Names of the agents whose chains have reached 1. An agent lands here exactly
        once, when its returning chain is first seen complete.
    """

    def __init__(
        self,
        episode_root: str,
        starts: dict[str, int],
        *,
        servers: str | list[str] = DEFAULT_NATS_SERVER,
        timeout: float | None = None,
    ) -> None:
        super().__init__(episode_root, *starts.keys(), servers=servers, timeout=timeout)
        # Subscribe to every agent's reply subject via one wildcard, in addition to the
        # environment's own command subject that the base class already claims.
        self.subjects.append(f"{episode_root}.{ENVIRONMENT}.{REPLIES}.*")
        self.starts = dict(starts)
        self.chains: dict[str, Chain] = {name: Chain(numbers=[n]) for name, n in starts.items()}
        self.finished: set[str] = set()

    async def start(self) -> None:
        """Start the episode, then hand each agent its starting :class:`Chain`.

        Calls the base :class:`~freeagent.sdk.Environment` start (connect + broadcast
        :class:`~freeagent.sdk.message.StartEntity`) so every agent's run loop is live before any
        chain is sent, then seeds each agent with its starting chain via :meth:`send_chain`.
        """
        await super().start()
        for name in self.agents:
            await self.send_chain(name)

    async def send_chain(self, agent: str) -> None:
        """Send an agent its current chain for it to extend by one step.

        :param agent: The name of the agent to send to; must be one of this episode's agents.
        """
        await self.request(f"{self.episode_root}.{AGENTS}.{agent}", self.chains[agent])

    def agent_of(self, subject: str) -> str:
        """Extract the agent name from a reply subject.

        Agents reply on ``{episode_root}.environment.replies.{agent}``; the agent name is the final
        subject token.

        :param subject: The subject a reply arrived on.
        :return: The agent name (the subject's last dot-delimited token).
        """
        return subject.rsplit(".", 1)[-1]

    def record(self, agent: str, chain: Chain) -> bool:
        """Record an agent's returned chain and report whether it just finished.

        Updates :attr:`chains` for ``agent`` and, if the chain has reached 1 and the agent wasn't
        already finished, adds it to :attr:`finished`. Pure bookkeeping -- no I/O -- so the
        completion logic is unit-testable without a server.

        :param agent: The agent whose chain this is.
        :param chain: The extended chain the agent returned.
        :return: ``True`` if this chain completed the agent's work for the first time, else
            ``False`` (either the chain isn't complete, or the agent was already finished).
        """
        self.chains[agent] = chain
        if is_complete(chain) and agent not in self.finished:
            self.finished.add(agent)
            return True
        return False

    def episode_complete(self) -> bool:
        """Report whether every agent in the episode has finished.

        :return: ``True`` once :attr:`finished` covers all of :attr:`agents`; ``False`` while any
            agent's chain is still running.
        """
        return self.finished == set(self.agents)

    async def handle_incoming_message(self, msg: Msg) -> None:
        """Handle a message on one of the environment's subjects.

        A :class:`Chain` arriving on an agent's reply subject is the environment's cue to judge that
        agent: it is acked immediately (ack-then-counter-request), recorded, and then either the
        agent is stopped (its chain reached 1) or sent the chain back to extend again. When the stop
        that just landed was the episode's last, the environment publishes
        :class:`~freeagent.sdk.message.EpisodeComplete` and stops itself. Anything that isn't a
        :class:`Chain` on a reply subject is acked and otherwise ignored -- the environment is not a
        general command target.

        :param msg: The NATS message that arrived on a subscribed subject.
        """
        message = Message.try_decode(msg.data)
        if not isinstance(message, Chain):
            await msg.respond(b"")
            return
        agent = self.agent_of(msg.subject)
        # Ack first: the reply carries no work, only receipt (ADR-0008). The agent's request
        # unblocks here, and the environment does its judging afterwards.
        await msg.respond(Ack().to_bytes())
        just_finished = self.record(agent, message)
        if just_finished:
            await self.stop_agent(agent)
            if self.episode_complete():
                await self.stop()
        else:
            await self.send_chain(agent)
