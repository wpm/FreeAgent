"""Base class for Free Agent agent processes.

An agent is an independent process that joins a Free Agent application by
connecting to NATS and subscribing to a set of subjects. Subclasses implement
:meth:`callback` to handle the messages that arrive on those subjects.
"""

from __future__ import annotations

import asyncio
from abc import ABC, abstractmethod
from asyncio import Queue, Task
from contextlib import suppress
from enum import StrEnum

import nats
from nats.aio.client import Client
from nats.aio.msg import Msg
from nats.aio.subscription import Subscription
from pydantic import BaseModel


class MessageType(StrEnum):
    PERCEPTION = "PERCEPTION"
    THOUGHT = "THOUGHT"
    EVENT = "EVENT"


class Message(BaseModel):
    type: MessageType


class Agent(ABC):
    """An independent process in a Free Agent application.

    Subclass it and implement :meth:`process` to handle incoming messages.
    :meth:`start` connects to NATS, subscribes to ``subjects`` (which feeds
    decoded messages onto :attr:`queue`), and launches the :meth:`run` loop that
    drains the queue into :meth:`process`. :meth:`stop` cancels that loop,
    unsubscribes, and disconnects.

    :param name: Identifier for this agent, used as the queue group when
        subscribing so multiple instances load-balance.
    :param subjects: NATS subjects this agent subscribes to.
    :param servers: NATS server URL(s) to connect to.
    """

    def __init__(
        self,
        name: str,
        *subjects: str,
        servers: str | list[str] = "nats://localhost:4222",
    ) -> None:
        self.name = name
        self.subjects = subjects
        self.servers = servers
        self.client: Client | None = None
        self.subscriptions: list[Subscription] = []
        self.queue = Queue[Message]()
        self.task: Task[None] | None = None

    async def start(self) -> None:
        """Connect to NATS, subscribe to the subjects, and start the run loop."""
        client = await nats.connect(self.servers)
        self.client = client
        for subject in self.subjects:
            subscription = await client.subscribe(subject, queue=self.name, cb=self.callback)
            self.subscriptions.append(subscription)
        self.task = asyncio.create_task(self.run())

    async def stop(self) -> None:
        """Stop the run loop, unsubscribe from the subjects, and disconnect."""
        if self.task is not None:
            self.task.cancel()
            with suppress(asyncio.CancelledError):
                await self.task
            self.task = None
        for subscription in self.subscriptions:
            await subscription.unsubscribe()
        self.subscriptions.clear()
        if self.client is not None:
            await self.client.close()
            self.client = None

    async def callback(self, message: Msg) -> None:
        """Decode a NATS message into a :class:`Message` and enqueue it.

        :param message: The NATS message that arrived on a subscribed subject.
        """
        await self.queue.put(Message.model_validate_json(message.data))

    async def run(self) -> None:
        """Process queued messages until cancelled.

        Pulls each :class:`Message` off :attr:`queue` and hands it to
        :meth:`process`, marking the queue task done afterwards. Loops forever;
        cancel the task running it to stop.
        """
        while True:
            message = await self.queue.get()
            try:
                await self.process(message)
            finally:
                self.queue.task_done()

    @abstractmethod
    async def process(self, message: Message) -> None:
        """Handle one decoded message off the queue.

        Subclasses implement this to give the agent its behaviour.

        :param message: The message pulled from :attr:`queue`.
        """
        raise NotImplementedError
