"""Fakes for the API's unit tests: the NATS client, worker process, and message shapes.

Each fake satisfies one of the protocols :mod:`freeagent.api.episodes` defines for injection
(:class:`~freeagent.api.episodes.NatsClient`, :class:`~freeagent.api.episodes.WorkerProcess`), so
the unit tier exercises the manager's real logic with no server and no subprocesses. The integration
tier (``test_integration.py``) swaps these for the real things.
"""

from __future__ import annotations

import subprocess

from freeagent.api.episodes import MessageHandler


class FakeSubscription:
    """Records the subject and callback it was created with, and whether it was unsubscribed."""

    def __init__(self, subject: str, cb: MessageHandler) -> None:
        self.subject = subject
        self.cb = cb
        self.unsubscribe_calls = 0

    async def unsubscribe(self) -> None:
        self.unsubscribe_calls += 1


class FakeNatsClient:
    """A stand-in for the manager's NATS client that records subscriptions, flushes, and closes."""

    def __init__(self) -> None:
        self.subscriptions: list[FakeSubscription] = []
        self.flush_calls = 0
        self.close_calls = 0

    async def subscribe(self, subject: str, *, cb: MessageHandler) -> FakeSubscription:
        subscription = FakeSubscription(subject, cb)
        self.subscriptions.append(subscription)
        return subscription

    async def flush(self) -> None:
        self.flush_calls += 1

    async def close(self) -> None:
        self.close_calls += 1


class FakeProcess:
    """A stand-in for a worker subprocess whose exit code the test scripts.

    :param returncode: The exit code :meth:`poll` reports, or ``None`` for a still-running worker.
    :param stubborn: If ``True``, the first :meth:`wait` after a terminate raises
        :class:`subprocess.TimeoutExpired`, exercising the manager's kill escalation.
    """

    def __init__(self, returncode: int | None = None, *, stubborn: bool = False) -> None:
        self.returncode = returncode
        self.stubborn = stubborn
        self.terminate_calls = 0
        self.kill_calls = 0

    def poll(self) -> int | None:
        return self.returncode

    def terminate(self) -> None:
        self.terminate_calls += 1
        if not self.stubborn:
            self.returncode = -15

    def kill(self) -> None:
        self.kill_calls += 1
        self.returncode = -9

    def wait(self, timeout: float | None = None) -> int:
        if self.returncode is None:
            raise subprocess.TimeoutExpired(cmd="fake-worker", timeout=timeout or 0.0)
        return self.returncode


class FakeMsg:
    """The two attributes of a NATS message the monitor callback reads."""

    def __init__(self, subject: str, data: bytes) -> None:
        self.subject = subject
        self.data = data
