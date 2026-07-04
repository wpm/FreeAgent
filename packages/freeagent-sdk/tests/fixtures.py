from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Self

from freeagent.sdk import Agent
from freeagent.sdk.message import Ack, Command, Message
from nats.aio.msg import Msg


class Product(Message):
    x: float
    y: float
    x_times_y: float | None = None

    def __call__(self) -> Self:
        return self.model_copy(update={"x_times_y": self.x * self.y})

    def __str__(self) -> str:
        if self.x_times_y is not None:
            s = f" = {self.x_times_y}"
        else:
            s = ""
        return f"{self.x} · {self.y}{s}"


class FakeSubscription:
    """Records whether it was unsubscribed, and how many times."""

    def __init__(self, subject: str, cb: Handler) -> None:
        self.subject = subject
        self.cb = cb
        self.unsubscribe_calls = 0

    async def unsubscribe(self) -> None:
        self.unsubscribe_calls += 1


class FakeClient:
    """A stand-in for nats.aio.client.Client that records interactions."""

    def __init__(self) -> None:
        self.subscriptions: list[FakeSubscription] = []
        self.closed = False
        self.close_calls = 0
        self.requests: list[tuple[str, bytes]] = []

    async def subscribe(self, subject: str, cb: Handler | None = None) -> FakeSubscription:
        assert cb is not None
        sub = FakeSubscription(subject, cb)
        self.subscriptions.append(sub)
        return sub

    async def request(self, subject: str, payload: bytes, **_: object) -> FakeMsg:
        """Record the request and reply with a bare Ack, as a real agent would."""
        self.requests.append((subject, payload))
        return FakeMsg.for_message(Ack())

    async def close(self) -> None:
        self.closed = True
        self.close_calls += 1


class FakeMsg:
    """A stand-in for nats.aio.msg.Msg carrying a raw payload."""

    def __init__(self, data: bytes, reply: str = "") -> None:
        self.data = data
        self.reply = reply
        self.responses: list[bytes] = []

    @classmethod
    def for_message(cls, message: Message, reply: str = "") -> FakeMsg:
        """Build a FakeMsg carrying the given message, optionally as a NATS request."""
        return cls(message.to_bytes(), reply=reply)

    async def respond(self, data: bytes) -> None:
        self.responses.append(data)


class Ping(Message):
    """A plain, in-domain message used to exercise the queue."""

    label: str = ""


class Shout(Command):
    """An application-defined command, used to exercise process_command()."""

    label: str = ""


class RecordingAgent(Agent):
    """A concrete agent whose process_message() and process_command() record what they handle."""

    def __init__(self, name: str, *subjects: str) -> None:
        super().__init__("episode-root", name, *subjects)
        self.processed: list[Message] = []
        self.commands: list[Command] = []

    async def process_message(self, message: Message) -> None:
        self.processed.append(message)

    async def process_command(self, msg: Msg, command: Command) -> None:
        self.commands.append(command)


Handler = Callable[[Msg], Awaitable[None]]
