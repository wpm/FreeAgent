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
from typing import final

import nats
from nats.aio.client import Client
from nats.aio.msg import Msg
from nats.aio.subscription import Subscription
from pydantic import BaseModel

STOP_AGENT = "STOP"


class Message(BaseModel):
    type: str


class Agent(ABC):
    """An independent process in a Free Agent application.

    Subclass it and implement :meth:`process` to handle incoming messages.
    :meth:`start` connects to NATS, subscribes to ``subjects`` (which feeds
    decoded messages onto :attr:`queue`), and launches a run loop that drains
    the queue into :meth:`process`.

    The agent shuts down either when
    :meth:`stop` is called or when a STOP message reaches the queue; both paths
    unsubscribe and disconnect. The loop and shutdown sequence are final, so
    subclasses cannot change how the agent stops.

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

    @final
    async def start(self) -> None:
        """Connect to NATS, subscribe to the subjects, and start the run loop."""
        client = await nats.connect(self.servers)
        self.client = client
        for subject in self.subjects:
            subscription = await client.subscribe(subject, queue=self.name, cb=self.callback)
            self.subscriptions.append(subscription)
        self.task = asyncio.create_task(self.__run())

    @final
    async def stop(self) -> None:
        """Stop the run loop, unsubscribe from the subjects, and disconnect.

        Safe to call more than once and from outside the loop. A STOP message on
        the queue triggers the same teardown from within the loop.
        """
        if self.task is not None and self.task is not asyncio.current_task():
            self.task.cancel()
            with suppress(asyncio.CancelledError):
                await self.task
        await self.__teardown()

    async def callback(self, message: Msg) -> None:
        """Decode a NATS message into a :class:`Message` and enqueue it.

        :param message: The NATS message that arrived on a subscribed subject.
        """
        await self.queue.put(Message.model_validate_json(message.data))

    @final
    async def __run(self) -> None:
        """Drain the queue into :meth:`process` until a STOP message arrives.

        A STOP message halts the loop the moment it is dequeued (messages ahead
        of it are still processed) and tears the agent down. This shutdown path
        cannot be overridden by subclasses.
        """
        while True:
            message = await self.queue.get()
            try:
                if message.type == STOP_AGENT:
                    break
                await self.process(message)
            finally:
                self.queue.task_done()
        await self.__teardown()

    @final
    async def __teardown(self) -> None:
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

    @abstractmethod
    async def process(self, message: Message) -> None:
        """Handle one decoded message off the queue.

        Subclasses implement this to give the agent its behaviour.

        :param message: The message pulled from :attr:`queue`.
        """
        raise NotImplementedError
