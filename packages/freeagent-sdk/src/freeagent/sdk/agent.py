"""Base class for Free Agent agent processes.

An agent is an independent process that joins a Free Agent application by
connecting to NATS and subscribing to a set of subjects. Subclasses implement
:meth:`callback` to handle the messages that arrive on those subjects.
"""

from __future__ import annotations

from asyncio import Queue
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


class Agent:
    """An independent process in a Free Agent application.

    Subclass it and override :meth:`callback` to handle incoming messages.
    :meth:`start` connects to NATS and subscribes to ``subjects``; :meth:`stop`
    unsubscribes and disconnects.

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

    async def start(self) -> None:
        """Connect to NATS and subscribe to the agent's subjects."""
        client = await nats.connect(self.servers)
        self.client = client
        for subject in self.subjects:
            subscription = await client.subscribe(subject, queue=self.name, cb=self.callback)
            self.subscriptions.append(subscription)

    async def stop(self) -> None:
        """Unsubscribe from the subjects and disconnect from NATS."""
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
