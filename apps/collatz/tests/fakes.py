"""Minimal NATS stand-ins for driving Collatz entities without a server.

The unit tests exercise Collatz's *logic* -- the Collatz step, the environment's completion judgment
and per-agent bookkeeping -- not real wire delivery (that is the integration suite's job). To reach
:meth:`~freeagent.app.collatz.entity.CollatzEnvironment.handle_incoming_message` without a broker,
these fakes record what an entity sends (requests, publishes, responses) and reply to every request
with a bare :class:`~freeagent.sdk.message.Ack`, exactly as a real agent's ack-then-work handler
would. They are deliberately tiny and local to this package rather than shared with the SDK's own
test fakes.
"""

from __future__ import annotations

from freeagent.sdk.message import Ack, Message


class FakeMsg:
    """A stand-in for :class:`nats.aio.msg.Msg`: carries a payload and records responses.

    :ivar data: The raw message bytes, as they would arrive over NATS.
    :ivar subject: The subject the message arrived on -- what the environment reads the agent name
        off of.
    :ivar responses: Every payload passed to :meth:`respond`, in order, for a test to assert the
        ack.
    """

    def __init__(self, data: bytes, subject: str = "") -> None:
        self.data = data
        self.subject = subject
        self.responses: list[bytes] = []

    @classmethod
    def for_message(cls, message: Message, subject: str = "") -> FakeMsg:
        """Build a :class:`FakeMsg` carrying ``message`` on ``subject``.

        :param message: The message to serialize into the fake's payload.
        :param subject: The subject the fake message arrives on.
        :return: A fake message ready to hand to a handler.
        """
        return cls(message.to_bytes(), subject=subject)

    async def respond(self, data: bytes) -> None:
        """Record a reply payload, as :class:`nats.aio.msg.Msg` would send one.

        :param data: The reply bytes.
        """
        self.responses.append(data)


class FakeClient:
    """A stand-in for :class:`nats.aio.client.Client` that records interactions.

    Assigned to an entity's ``client`` so its ``request``/``publish``/``stop`` reach these recorders
    instead of a real connection. Every request is answered with a bare
    :class:`~freeagent.sdk.message.Ack`, matching a real counterpart under the ack-then-work shape.

    :ivar requests:``(subject, payload)`` for every request sent, in order.
    :ivar published:``(subject, payload)`` for every fire-and-forget publish, in order.
    :ivar closed: Whether :meth:`close` has been called.
    """

    def __init__(self) -> None:
        self.requests: list[tuple[str, bytes]] = []
        self.published: list[tuple[str, bytes]] = []
        self.closed = False

    async def request(
        self, subject: str, payload: bytes, timeout: float = 0.5, **_: object
    ) -> FakeMsg:
        """Record a request and reply with a bare :class:`~freeagent.sdk.message.Ack`.

        :param subject: The request's subject.
        :param payload: The request's payload bytes.
        :param timeout: Ignored; accepted to match the real client's signature.
        :return: A fake reply message carrying an :class:`~freeagent.sdk.message.Ack`.
        """
        self.requests.append((subject, payload))
        return FakeMsg.for_message(Ack())

    async def publish(self, subject: str, payload: bytes) -> None:
        """Record a fire-and-forget publish.

        :param subject: The publish subject.
        :param payload: The payload bytes.
        """
        self.published.append((subject, payload))

    async def close(self) -> None:
        """Record that the connection was closed."""
        self.closed = True
