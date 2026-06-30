from __future__ import annotations

import asyncio
import contextlib
from asyncio import Queue, Task, gather, wait_for
from typing import final

import nats
from nats.aio.client import Client
from nats.aio.msg import Msg
from nats.aio.subscription import Subscription
from pydantic import BaseModel

# The name of the environment entity or its subject in an episode.
ENVIRONMENT = "environment"
# A subject beneath each agent subject in an episode for receiving control messages.
CONTROL_SUBJECT = "control"
# Start an agent.
START_AGENT = "START"
# Stop an agent.
STOP_AGENT = "STOP"
# Acknowledge a control command.
ACK = "ack"


DEFAULT_NATS_SERVER = "nats://localhost:4222"


class Message(BaseModel):
    type: str


class Ack(Message):
    """Reply to a control command, reporting whether the agent's run loop is active."""

    type: str = ACK
    running: bool


class Entity:
    """Base class for Free Agent agent processes.

    An entity is an independent process that joins a Free Agent application by connecting to NATS
    and subscribing to a set of subjects.
    """

    def __init__(
        self,
        episode_root: str,
        name: str,
        *subjects: str,
        servers: str | list[str] = DEFAULT_NATS_SERVER,
    ) -> None:
        self.episode_root = episode_root
        self.name = name
        self.subjects = [
            f"{episode_root}.{name}.{subject}" for subject in list(subjects) + [CONTROL_SUBJECT]
        ]
        self.servers = servers
        self.client: Client | None = None
        self.subscriptions: list[Subscription] = []

    async def start(self) -> None:
        """Connect to NATS, subscribe to the subjects, and start the run loop.

        This is idempotent.
        """
        if self.client is not None:
            return
        client = await nats.connect(self.servers)
        self.client = client
        for subject in self.subjects:
            subscription = await client.subscribe(subject, queue=self.name, cb=self.callback)
            self.subscriptions.append(subscription)

    async def stop(self) -> None:
        """Unsubscribe from all subjects and disconnect from NATS.

        This is idempotent.
        """
        if self.client is None:
            return
        await gather(*[subscription.unsubscribe() for subscription in self.subscriptions])
        self.subscriptions.clear()
        if self.client is not None:
            await self.client.close()
            self.client = None

    async def callback(self, msg: Msg) -> None:
        """Handle an incoming NATS message.

        :param msg: The NATS message that arrived on a subscribed subject.
        """
        pass

    async def request(self, subject: str, message: Message) -> Msg:
        if self.client is None:
            await self.start()
        assert self.client is not None
        return await self.client.request(
            subject,
            message.model_dump_json().encode(),
        )

    async def publish(self, subject: str, message: Message) -> None:
        if self.client is None:
            await self.start()
        assert self.client is not None
        await self.client.publish(
            subject,
            message.model_dump_json().encode(),
        )


class Environment(Entity):
    """
    For every episode there is an environment that manages that episode's agents' lifecycles and
    enforces a shared reality among them.
    """

    def __init__(
        self,
        episode_root: str,
        *agents: str,
        servers: str | list[str] = DEFAULT_NATS_SERVER,
        timeout: float | None = None,
    ) -> None:
        super().__init__(episode_root, ENVIRONMENT, ENVIRONMENT, servers=servers)
        self.agents = agents
        self.timeout = timeout

    async def start(self) -> None:
        await super().start()
        await wait_for(self.broadcast_to_agents(Message(type=START_AGENT)), self.timeout)

    async def stop(self) -> None:
        if self.client is None:
            return
        try:
            await wait_for(self.broadcast_to_agents(Message(type=STOP_AGENT)), self.timeout)
        finally:
            await super().stop()

    async def broadcast_to_agents(self, message: Message) -> None:
        await gather(
            *[
                self.request(
                    f"{self.episode_root}.{agent}.{CONTROL_SUBJECT}",
                    message,
                )
                for agent in self.agents
            ]
        )


class Agent(Entity):
    """Agents are independent entities in a Free Agent application that interact with each other
    within a shared Environment.

    Subclass it and implement :meth:`process` to handle incoming messages.
    :meth:`start` connects to NATS, subscribes to ``subjects`` (which feeds
    decoded messages onto :attr:`queue`), and launches a run loop that drains
    the queue into :meth:`process`.

    In addition to specified subjects, all agents subscribe to a `CONTROL_SUBJECT`
    on which they receive requests from the environment such as lifecycle
    commands. All agents support `START_AGENT` and `STOP_AGENT` messages.
    `START_AGENT` starts the agent's message queue and has to be receive before the agent will do
    anything else.
    `STOP_AGENT` shuts down all activity.
    Both of these messages are idempotent.
    Derived classes may handle other control messages.

    The agent shuts down either when
    :meth:`stop` is called or when a STOP message reaches the queue; both paths
    unsubscribe and disconnect. The loop and shutdown sequence are final, so
    subclasses cannot change how the agent stops.

    :param episode_root: The root NATS subject for this agent's episode.
    :param name: Identifier for this agent
    :param subjects: NATS subjects this agent subscribes to.
    :param servers: NATS server URL(s) to connect to.
    """

    def __init__(
        self,
        episode_root: str,
        name: str,
        *subjects: str,
        servers: str | list[str] = "nats://localhost:4222",
    ) -> None:
        if name == ENVIRONMENT:
            raise ValueError(f'"{name}" is reserved entity name.')
        if CONTROL_SUBJECT in subjects:
            raise ValueError(f'"{CONTROL_SUBJECT}" is a reserved subject.')
        self.subjects = [
            f"{episode_root}.{name}.{subject}" for subject in list(subjects) + [CONTROL_SUBJECT]
        ]
        super().__init__(episode_root, name, *subjects, servers=servers)
        self.queue: Queue[Message] = Queue()
        self.task: Task[None] | None = None

    @final
    async def callback(self, msg: Msg) -> None:
        """Decode a NATS message into a :class:`Message` and enqueue it.

        :param msg: The NATS message that arrived on a subscribed subject.
        """
        message = Message.model_validate_json(msg.data)
        if msg.subject == CONTROL_SUBJECT:
            await self.process_command(msg, message)
        else:
            await self.queue.put(message)

    async def process_command(self, msg: Msg, command: Message) -> None:
        """Handle a control command and, if it was sent as a request, reply with an ack.

        :param msg: The original NATS message, used to reply to the requester.
        :param command: The decoded control command.
        """
        if command.type == START_AGENT:
            self.task = asyncio.create_task(self.run())
            if msg.reply:
                await msg.respond(Ack(running=True).model_dump_json().encode())
        else:
            if msg.reply:
                await msg.respond(Ack(running=False).model_dump_json().encode())
            if self.task is not None:
                self.task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await self.task
                self.task = None
            await self.stop()

    @final
    async def run(self) -> None:
        """Drain the queue into :meth:`process`."""
        while True:
            message = await self.queue.get()
            try:
                await self.process_message(message)
            finally:
                self.queue.task_done()

    async def process_message(self, message: Message) -> None:
        """Handle a message pulled from the queue.

        Subclasses implement this to give the agent its behaviour.

        :param message: The message pulled from :attr:`queue`.
        """
        pass
