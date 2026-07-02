"""Entities: independent processes that communicate over NATS.

An :class:`Entity` is a single Free Agent process. Entities never call each other directly; they
publish and subscribe to NATS subjects rooted at a shared ``episode_root``, exchanging
:class:`~freeagent.sdk.message.Message` instances encoded as JSON. Free Agent runs one *episode* at
a time, and within an episode there is exactly one :class:`Environment` and one or more
:class:`Agent` instances.

Each entity claims a subtree of the subject namespace under ``episode_root``:

- The :class:`Environment` subscribes under ``{episode_root}.environment``.
- Each :class:`Agent` named ``name`` subscribes under ``{episode_root}.agents.{name}``, plus
  whatever additional ``subjects`` it was constructed with.

The :class:`Environment` drives the episode's lifecycle: :meth:`Environment.start` connects and
then broadcasts a :class:`~freeagent.sdk.message.StartEntity` command to every agent; conversely
:meth:`Environment.stop` broadcasts :class:`~freeagent.sdk.message.StopEntity` before disconnecting
itself. Agents don't distinguish these lifecycle commands from ordinary traffic by subject; instead
:meth:`Agent.handle_incoming_message` pattern-matches on the decoded message's type, handling
:class:`~freeagent.sdk.message.Command` subclasses immediately and queuing everything else for
:meth:`Agent.process_message`.
"""

from __future__ import annotations

import asyncio
import contextlib
from asyncio import Queue, Task, gather, wait_for
from typing import final

import nats
from freeagent.sdk.message import Ack, Command, Message, StartEntity, StopEntity
from nats.aio.client import Client
from nats.aio.msg import Msg
from nats.aio.subscription import Subscription

DEFAULT_NATS_SERVER = "nats://localhost:4222"
"""Default NATS server URL used when an :class:`Entity` isn't given one explicitly."""

ENVIRONMENT = "environment"
"""Subject segment claimed by the :class:`Environment`."""

AGENTS = "agents"
"""Subject segment under which every :class:`Agent` subscribes."""


class Entity:
    """Base class for Free Agent processes.

    An entity is an independent Free Agent process that communicates with the rest of its episode
    over NATS. It tracks its own connection and subscriptions and knows how to tear both down; on
    its own it doesn't do anything with incoming messages; :meth:`handle_incoming_message` is a
    no-op here, so subclasses such as :class:`Agent` override it to add behavior.

    :param servers: NATS server URL(s) to connect to.
    :param episode_root: The root NATS subject for this entity's episode.
    :param subjects: NATS subjects this entity subscribes to, appended to ``episode_root``.
    """

    def __init__(
        self,
        servers: str | list[str],
        episode_root: str,
        *subjects: str,
    ) -> None:
        self.episode_root = episode_root
        self.subjects = [f"{episode_root}.{subject}" for subject in list(subjects)]
        self.servers = servers
        self.client: Client | None = None
        self.subscriptions: list[Subscription] = []

    async def start(self) -> None:
        """Connect to NATS and subscribe to :attr:`subjects`.

        Incoming messages on any of :attr:`subjects` are delivered to
        :meth:`handle_incoming_message`. This is idempotent: calling it again while already
        connected does nothing.
        """
        if self.client is not None:
            return
        client = await nats.connect(self.servers)
        self.client = client
        for subject in self.subjects:
            subscription = await client.subscribe(subject, cb=self.handle_incoming_message)
            self.subscriptions.append(subscription)

    async def stop(self) -> None:
        """Unsubscribe from all subjects and disconnect from NATS.

        This is idempotent: calling it again while already disconnected does nothing.
        """
        if self.client is None:
            return
        await gather(*[subscription.unsubscribe() for subscription in self.subscriptions])
        self.subscriptions.clear()
        await self.client.close()
        self.client = None

    async def handle_incoming_message(self, msg: Msg) -> None:
        """Handle an incoming NATS message.

        The base implementation does nothing; override in subclasses to give an entity behavior.
        Called as the subscription callback registered by :meth:`start` for every subject in
        :attr:`subjects`.

        :param msg: The NATS message that arrived on a subscribed subject.
        """
        pass

    async def request(self, subject: str, message: Message) -> Msg:
        """Send a message as a NATS request and wait for the reply.

        Connects first via :meth:`start` if not already connected.

        :param subject: The NATS subject to send the request to.
        :param message: The message to send.
        :return: The raw NATS reply message.
        """
        if self.client is None:
            await self.start()
        assert self.client is not None
        return await self.client.request(
            subject,
            message.model_dump_json().encode(),
        )


class Environment(Entity):
    """The environment manages an episode's agents' lifecycles and enforces a shared reality
    among them.

    There is exactly one :class:`Environment` per episode. :meth:`start` connects to NATS and then
    broadcasts a :class:`~freeagent.sdk.message.StartEntity` command to every agent named in
    ``agents``, via :meth:`broadcast_to_agents`; :meth:`stop` does the reverse, broadcasting
    :class:`~freeagent.sdk.message.StopEntity` before disconnecting itself.

    :param episode_root: The root NATS subject for this episode.
    :param agents: Names of the agents that participate in this episode.
    :param servers: NATS server URL(s) to connect to.
    :param timeout: Seconds to wait for every agent to acknowledge a broadcast
        :class:`~freeagent.sdk.message.StartEntity`/:class:`~freeagent.sdk.message.StopEntity`
        command, or ``None`` to wait indefinitely.
    """

    def __init__(
        self,
        episode_root: str,
        *agents: str,
        servers: str | list[str] = DEFAULT_NATS_SERVER,
        timeout: float | None = None,
    ) -> None:
        super().__init__(servers, episode_root, ENVIRONMENT)
        self.agents = agents
        self.timeout = timeout

    async def start(self) -> None:
        """Connect to NATS, then broadcast :class:`~freeagent.sdk.message.StartEntity` to every
        agent.

        Overrides :meth:`Entity.start` to add the broadcast; see that method for the connection
        behavior.
        """
        await super().start()
        await wait_for(self.broadcast_to_agents(StartEntity()), self.timeout)

    async def stop(self) -> None:
        """Broadcast :class:`~freeagent.sdk.message.StopEntity` to every agent, then disconnect.

        Overrides :meth:`Entity.stop`. Does nothing if not currently connected; otherwise
        broadcasts the stop command before disconnecting, even if the broadcast raises or times
        out.
        """
        if self.client is None:
            return
        try:
            await wait_for(self.broadcast_to_agents(StopEntity()), self.timeout)
        finally:
            await super().stop()

    async def broadcast_to_agents(self, message: Message, postfix: str | None = None) -> list[Msg]:
        """Send ``message`` as a NATS request to every agent in :attr:`agents`, in parallel.

        :param message: The message to send to each agent.
        :param postfix: If given, appended (as ``.postfix``) to each agent's subject, to target a
            specific subject an agent subscribes to rather than its bare name.
        :return: Each agent's raw NATS reply, in the same order as :attr:`agents`.
        """
        if postfix is None:
            postfix = ""
        else:
            postfix = f".{postfix}"
        return await gather(
            *[
                self.request(
                    f"{self.episode_root}.{agent}{postfix}",
                    message,
                )
                for agent in self.agents
            ]
        )


class Agent(Entity):
    """Agents are independent entities in a Free Agent application that interact with each other
    within a shared :class:`Environment`.

    Subclass it and implement :meth:`process_message` to give the agent its behavior. :meth:`start`
    (inherited from :class:`Entity`) connects to NATS and subscribes under
    ``{episode_root}.agents.{name}``, plus any additional ``subjects``; messages received there are
    decoded and dispatched by :meth:`handle_incoming_message`.

    :meth:`handle_incoming_message` tells messages apart by their pydantic type rather than by
    subject:

    - A :class:`~freeagent.sdk.message.StartEntity` command launches :meth:`run` as a background
      task, which drains :attr:`queue` into :meth:`process_message` one message at a time. An
      agent must receive this before it processes anything else.
    - A :class:`~freeagent.sdk.message.StopEntity` command cancels the run loop and tears the
      agent down, via :meth:`Entity.stop`: unsubscribing and disconnecting from NATS.
    - Any other :class:`~freeagent.sdk.message.Command` is dispatched to :meth:`process_command`,
      which subclasses may override to handle application-specific commands.
    - Anything else is pushed onto :attr:`queue` for :meth:`process_message` to handle in turn.

    Both :class:`~freeagent.sdk.message.StartEntity` and
    :class:`~freeagent.sdk.message.StopEntity` are idempotent, and both are replied to with an
    :class:`~freeagent.sdk.message.Ack` (when the incoming message was a NATS request) reporting
    whether the run loop is active after the command was handled.

    :param episode_root: The root NATS subject for this agent's episode.
    :param name: Identifier for this agent.
    :param subjects: Additional NATS subjects this agent subscribes to, beyond its own
        ``{episode_root}.agents.{name}``.
    :param servers: NATS server URL(s) to connect to.
    """

    def __init__(
        self,
        episode_root: str,
        name: str,
        *subjects: str,
        servers: str | list[str] = DEFAULT_NATS_SERVER,
    ) -> None:
        self.subjects = [f"{episode_root}.{AGENTS}.{name}.{subject}" for subject in list(subjects)]
        super().__init__(servers, episode_root, name, *subjects)
        self.queue: Queue[tuple[Msg | None, Message]] = Queue()
        self.task: Task[None] | None = None

    @final
    async def handle_incoming_message(self, msg: Msg) -> None:
        """Decode an incoming NATS message and dispatch it by type.

        Overrides :meth:`Entity.handle_incoming_message`. See the class docstring for how
        :class:`~freeagent.sdk.message.StartEntity`, :class:`~freeagent.sdk.message.StopEntity`,
        other :class:`~freeagent.sdk.message.Command` subclasses, and plain messages are each
        handled.

        :param msg: The NATS message that arrived on a subscribed subject.
        """
        message = Message.model_validate_json(msg.data)
        match message:
            case StartEntity():
                self.task = asyncio.create_task(self.run())
                await self.respond(msg, Ack())
            case StopEntity():
                if self.task is not None:
                    self.task.cancel()
                    with contextlib.suppress(asyncio.CancelledError):
                        await self.task
                    self.task = None
                await self.respond(msg, Ack())
                await self.stop()
            case Command():
                await self.process_command(msg, message)
            case _:
                await self.queue.put((msg, message))

    @final
    async def run(self) -> None:
        """Drain :attr:`queue`, passing each message to :meth:`process_message` in turn.

        Runs as a background task started by :meth:`handle_incoming_message` on
        :class:`~freeagent.sdk.message.StartEntity`, and cancelled on
        :class:`~freeagent.sdk.message.StopEntity`. Every message is replied to, either with
        whatever :meth:`process_message` returns or, if it returns ``None``, with a bare
        :class:`~freeagent.sdk.message.Ack`.
        """
        while True:
            msg, message = await self.queue.get()
            try:
                reply = await self.process_message(message) or Ack()
                await self.respond(msg, reply)
            finally:
                self.queue.task_done()

    async def process_command(self, msg: Msg, command: Command) -> None:
        """Handle a control command that isn't :class:`~freeagent.sdk.message.StartEntity` or
        :class:`~freeagent.sdk.message.StopEntity`.

        Those two are handled directly by :meth:`handle_incoming_message`; every other
        :class:`~freeagent.sdk.message.Command` subclass is dispatched here instead. The base
        implementation does nothing; subclasses override it to add application-specific commands.

        :param msg: The original NATS message, used to reply to the requester via :meth:`respond`.
        :param command: The decoded control command.
        """
        pass

    async def process_message(self, message: Message) -> Message | None:
        """Handle a message pulled from :attr:`queue`.

        Subclasses implement this to give the agent its behavior; it is called once per message,
        in the order the messages were received, by :meth:`run`.

        :param message: The message pulled from :attr:`queue`.
        :return: An optional reply message to send back to the sender. If ``None``, :meth:`run`
            sends a bare :class:`~freeagent.sdk.message.Ack` instead.
        """
        pass

    @staticmethod
    async def respond(msg: Msg | None, reply: Message) -> None:
        """Reply to a NATS request with a message, if it was one.

        :param msg: The original NATS message, or ``None`` if it wasn't a NATS request (e.g. it
            arrived via ordinary publish rather than request/reply), in which case this does
            nothing.
        :param reply: The message to send back to the requester.
        """
        if msg is None:
            return
        await msg.respond(reply.to_bytes())
