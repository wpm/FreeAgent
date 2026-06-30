"""Base class for Free Agent agent processes.

An agent is an independent process that joins a Free Agent application by
connecting to NATS and subscribing to a set of subjects. Subclasses implement
:meth:`callback` to handle the messages that arrive on those subjects.
"""

from __future__ import annotations

from abc import ABC, abstractmethod

import nats
from nats.aio.client import Client
from nats.aio.msg import Msg
from nats.aio.subscription import Subscription


class Agent(ABC):
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
        self._client: Client | None = None
        self._subscriptions: list[Subscription] = []

    async def start(self) -> None:
        """Connect to NATS and subscribe to the agent's subjects."""
        client = await nats.connect(self.servers)
        self._client = client
        for subject in self.subjects:
            subscription = await client.subscribe(subject, queue=self.name, cb=self.callback)
            self._subscriptions.append(subscription)

    async def stop(self) -> None:
        """Unsubscribe from the subjects and disconnect from NATS."""
        for subscription in self._subscriptions:
            await subscription.unsubscribe()
        self._subscriptions.clear()
        if self._client is not None:
            await self._client.close()
            self._client = None

    @abstractmethod
    async def callback(self, message: Msg) -> None:
        """Handle a message delivered to one of the agent's subjects.

        Subclasses override this. The default raises, so an agent that forgets
        to implement it fails loudly.

        :param message: The NATS message that arrived on a subscribed subject.
        """
        raise NotImplementedError
