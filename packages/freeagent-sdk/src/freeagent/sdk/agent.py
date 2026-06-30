"""Base class for Free Agent agent processes.

An agent is an independent process that joins a Free Agent application by connecting to NATS and
subscribing to a set of subjects. Subclasses implement :meth:`callback` to handle the messages that
arrive on those subjects.
"""

from __future__ import annotations

import asyncio
from abc import ABC, abstractmethod
from asyncio import Queue, Task
from contextlib import suppress
from typing import final

import nats
from nats.aio.client import Client
from nats.aio.msg import Msg
from nats.aio.subscription import Subscription
from pydantic import BaseModel

CONTROL_SUBJECT = "control"
START_AGENT = "START"
STOP_AGENT = "STOP"


class Message(BaseModel):
    type: str


class Agent(ABC):
    """An independent process in a Free Agent application.

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
        self.episode_root = episode_root
        self.name = name
        if CONTROL_SUBJECT in subjects:
            raise ValueError(f"{CONTROL_SUBJECT} is a reserved subject.")
        self.subjects = [
            f"{episode_root}.{name}.{subject}" for subject in list(subjects) + [CONTROL_SUBJECT]
        ]
        self.servers = servers
        self.client: Client | None = None
        self.subscriptions: list[Subscription] = []
        self.queue: Queue[tuple[Message, bool]] = Queue()
        self.task: Task[None] | None = None

    @final
    async def connect(self) -> None:
        """Connect to NATS, subscribe to the subjects, and start the run loop."""
        client = await nats.connect(self.servers)
        self.client = client
        for subject in self.subjects:
            subscription = await client.subscribe(subject, queue=self.name, cb=self.callback)
            self.subscriptions.append(subscription)

    @final
    async def close(self) -> None:
        """Stop the run loop, unsubscribe from the subjects, and disconnect.

        Safe to call more than once and from outside the loop. A STOP message on the queue triggers
        the same teardown from within the loop.
        """
        task = self.task
        if task is not None and task is not asyncio.current_task():
            task.cancel()
            with suppress(asyncio.CancelledError):
                await task
        await self.__stop()

    async def callback(self, msg: Msg) -> None:
        """Decode a NATS message into a :class:`Message` and enqueue it.

        :param msg: The NATS message that arrived on a subscribed subject.
        """
        is_control = msg.subject == CONTROL_SUBJECT
        message = Message.model_validate_json(msg.data)
        match is_control, message.type:
            case True, msg_type if msg_type == START_AGENT:
                self.task = asyncio.create_task(self.__start())
            case True, msg_type if msg_type == STOP_AGENT:
                await self.__stop()
            case _:
                await self.queue.put((message, is_control))

    @abstractmethod
    async def process(self, message: Message, is_control: bool) -> None:
        """Handle one decoded message off the queue.

        Subclasses implement this to give the agent its behaviour.

        :param message: The message pulled from :attr:`queue`.
        :param is_control: is this a control message from the environment?
        """
        raise NotImplementedError

    @final
    async def __start(self) -> None:
        """Drain the queue into :meth:`process` until a STOP message arrives.

        A STOP control message from the environment halts the loop the moment it is dequeued
        (messages ahead of it are still processed) and tears the agent down. This shutdown path
        cannot be overridden by subclasses.
        """
        while True:
            message, is_control = await self.queue.get()
            try:
                if is_control and message.type == STOP_AGENT:
                    break
                await self.process(message, is_control)
            finally:
                self.queue.task_done()
        await self.__stop()

    @final
    async def __stop(self) -> None:
        """Unsubscribe from every subject and disconnect from NATS.

        Idempotent: clears the task handle, subscriptions, and client so it can
        be called from both :meth:`stop` and the STOP path in :meth:`__run`.
        """
        self.task = None
        for subscription in self.subscriptions:
            await subscription.unsubscribe()
        self.subscriptions.clear()
        if self.client is not None:
            await self.client.close()
            self.client = None
