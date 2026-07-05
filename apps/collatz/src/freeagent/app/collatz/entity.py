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

import asyncio
import contextlib

from freeagent.app.collatz.message import Chain, extend, is_complete
from freeagent.sdk import Agent, Environment
from freeagent.sdk.entity import AGENTS, DEFAULT_NATS_SERVER, ENVIRONMENT
from freeagent.sdk.message import Ack, Message
from nats.aio.msg import Msg
from nats.errors import NoRespondersError
from nats.errors import TimeoutError as NatsTimeoutError

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
    :class:`~freeagent.sdk.message.Ack`, and the real result travels as a fresh request in the
    other direction. The agent makes no completion judgment of its own; it extends and returns, and
    the environment decides when the agent is finished (and stops it).

    The counter-request is sent *without waiting for its reply* -- it is launched as a background
    task, tracked in :attr:`pending`, so :meth:`process_message` returns at once and the run loop
    acks the environment's request immediately. This is the whole point of the ack-then-counter
    shape (ADR-0008), and getting it wrong deadlocks a multi-agent episode: NATS delivers a
    subscription's messages one at a time, so the environment's single reply subscription processes
    one agent's returned chain at a time, and *its* handler sends the next chain back with a
    :meth:`~freeagent.sdk.entity.Environment.request` that blocks until this agent's run loop acks.
    If this agent's run loop only acked *after* its own counter-request round-tripped, that ack
    would wait on the environment -- itself busy waiting on this ack -- and every other agent would
    stall behind the environment's blocked subscription. Not awaiting the counter-request's reply
    here breaks that cycle: the ack goes out promptly and the environment's subscription is free to
    service the next agent.

    :param episode_root: The root NATS subject for this agent's episode.
    :param name: This agent's identifier; also the last token of the environment reply subject it
        sends extended chains to.
    :param servers: NATS server URL(s) to connect to.

    :ivar pending: The in-flight counter-request tasks, each launched by :meth:`process_message`
        and removed on completion. Held so they aren't garbage-collected mid-flight (asyncio keeps
        only a weak reference to a bare task) and so :meth:`stop` can cancel any still outstanding
        at teardown.
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
        self.pending: set[asyncio.Task[None]] = set()

    async def process_message(self, message: Message) -> Message | None:
        """Extend an incoming :class:`Chain` and send the result back to the environment.

        Non-:class:`Chain` messages are ignored (acknowledged with the run loop's default
        :class:`~freeagent.sdk.message.Ack`). For a :class:`Chain`, the extended chain is sent to
        the environment's per-agent reply subject as a *background* counter-request (see the class
        docstring for why it must not be awaited here); this method returns ``None`` at once, so the
        run loop's reply to the environment stays a bare :class:`~freeagent.sdk.message.Ack`,
        keeping the work off the reply per ADR-0008.

        :param message: The message pulled from the agent's queue.
        :return: Always ``None``: the run loop then replies with a bare
            :class:`~freeagent.sdk.message.Ack`, and the extended chain has gone out as a separate,
            not-awaited counter-request.
        """
        if isinstance(message, Chain):
            task = asyncio.create_task(self._send_extended(message))
            self.pending.add(task)
            task.add_done_callback(self.pending.discard)
        return None

    async def _send_extended(self, chain: Chain) -> None:
        """Counter-request the environment with ``chain`` extended by one Collatz step.

        Run as a background task by :meth:`process_message`. Once the episode is ending the
        environment stops this agent, unsubscribing its reply subject; a counter-request already in
        flight then has no responder, the normal end-of-chain race rather than an error, so
        :class:`~nats.errors.NoRespondersError` and a request :class:`TimeoutError` are swallowed.

        :param chain: The chain this agent was handed; the extended chain is what gets sent.
        """
        with contextlib.suppress(NoRespondersError, NatsTimeoutError):
            await self.request(self.reply_subject, extend(chain))

    async def stop(self) -> None:
        """Cancel any outstanding counter-requests, then tear the agent down.

        Overrides :meth:`~freeagent.sdk.entity.Entity.stop` to first cancel the background
        counter-request tasks in :attr:`pending` -- when the environment stops this agent its reply
        subject goes away, so an in-flight counter-request would otherwise hang until it timed
        out -- and then run the base teardown (unsubscribe + disconnect). Idempotent, matching the
        base: with nothing pending and no client it does nothing.
        """
        for task in list(self.pending):
            task.cancel()
        for task in list(self.pending):
            with contextlib.suppress(asyncio.CancelledError):
                await task
        await super().stop()


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
        self.replies_root = f"{episode_root}.{ENVIRONMENT}.{REPLIES}"
        self.subjects.append(f"{self.replies_root}.*")
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

        A :class:`Chain` arriving on an agent's reply subject (``{replies_root}.{agent}``) is the
        environment's cue to judge that agent: it is acked immediately (ack-then-counter-request),
        recorded, and then either the agent is stopped (its chain reached 1) or sent the chain back
        to extend again. When the stop that just landed was the episode's last, the environment
        publishes :class:`~freeagent.sdk.message.EpisodeComplete` and stops itself.

        Every other message is left alone: the environment also subscribes (via the base class) to
        its own ``{episode_root}.environment`` subject, on which it *self-receives* the
        fire-and-forget :class:`~freeagent.sdk.message.EpisodeComplete` it publishes during
        teardown. That message carries no reply subject, so replying to it would raise; the
        environment only ever :meth:`~freeagent.sdk.entity.Agent.respond`-acks a message that is an
        actual request (has a reply subject), and only chains on reply subjects drive the game.

        :param msg: The NATS message that arrived on a subscribed subject.
        """
        if not msg.subject.startswith(f"{self.replies_root}."):
            # Not a per-agent reply -- e.g. the environment's own EpisodeComplete on its
            # {episode_root}.environment subject. Ack only if a request; never drive the game.
            await self._ack_if_request(msg)
            return
        message = Message.try_decode(msg.data)
        if not isinstance(message, Chain):
            # A reply that isn't a chain (unknown or malformed type). Ack it if it was a request,
            # but do not advance the game -- an undecodable reply can't be judged.
            await self._ack_if_request(msg)
            return
        agent = self.agent_of(msg.subject)
        # Ack first: the reply carries no work, only receipt (ADR-0008). The agent's request
        # unblocks here, and the environment does its judging afterwards.
        await self._ack_if_request(msg, Ack())
        just_finished = self.record(agent, message)
        if just_finished:
            await self.stop_agent(agent)
            if self.episode_complete():
                await self.stop()
        else:
            await self.send_chain(agent)

    @staticmethod
    async def _ack_if_request(msg: Msg, reply: Message | None = None) -> None:
        """Reply to ``msg`` only if it was a NATS request (carries a reply subject).

        A fire-and-forget publish (such as the environment's self-received
        :class:`~freeagent.sdk.message.EpisodeComplete`) has no reply subject, and replying to it
        raises; this makes responding a no-op in that case, mirroring the guard in
        :meth:`~freeagent.sdk.entity.Agent.respond`.

        :param msg: The NATS message to (maybe) reply to.
        :param reply: The message to reply with; a bare :class:`~freeagent.sdk.message.Ack` payload
            of ``b""`` is sent when ``None``.
        """
        if not msg.reply:
            return
        await msg.respond(b"" if reply is None else reply.to_bytes())

    async def stop(self) -> None:
        """Tear the episode down, broadcasting :class:`~freeagent.sdk.message.StopEntity` only to
        agents not already stopped.

        Overrides :meth:`~freeagent.sdk.entity.Environment.stop`, which broadcasts the teardown to
        *every* agent. On the completing path each agent has already been stopped individually by
        :meth:`~freeagent.sdk.entity.Environment.stop_agent` as its chain reached 1, so those agents
        have unsubscribed and disconnected; re-broadcasting to them would hit subjects with no
        responder. This narrows :attr:`~freeagent.sdk.entity.Environment.agents` to the still-live
        agents for the duration of the base teardown -- reusing all of its behavior (the
        :class:`~freeagent.sdk.message.EpisodeComplete` end marker, ordering, and disconnect) and
        only changing *who* the :class:`~freeagent.sdk.message.StopEntity` broadcast reaches -- then
        restores the full agent list.

        :return: ``None``.
        """
        live = tuple(agent for agent in self.agents if agent not in self.stopped_agents)
        full_agents = self.agents
        self.agents = live
        try:
            await super().stop()
        finally:
            self.agents = full_agents
