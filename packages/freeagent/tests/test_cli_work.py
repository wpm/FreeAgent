"""Unit tests for the manifest-pulling worker (``free-agent work``).

These exercise the worker's *loop logic* without a NATS server and without real
child processes: a fake pull subscription supplies manifests, and the child
factory is stubbed with fakes that model the three outcomes the worker must
distinguish -- a child that comes up, a child that dies in its readiness window
(transport failure), and a straggler that ignores cooperative shutdown.

The real "two workers, no duplicates", "failed child redelivers", and
"straggler hard-killed" acceptance criteria run against real NATS and real
processes in :mod:`test_cli_work_integration`, gated behind ``FREEAGENT_SLOW_TEST``.
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

from freeagent import Manifest, MemoryTransport
from freeagent.cli.child import EXIT_ENDED, EXIT_TRANSPORT
from freeagent.cli.work import Worker
from freeagent.workqueue import (
    WORK_QUEUE_CONSUMER,
    WORK_QUEUE_STREAM,
    WORK_QUEUE_SUBJECTS,
    work_subject,
)

if TYPE_CHECKING:
    from collections.abc import Sequence

NATS_URL = "nats://localhost:4222"


def _manifest(agent_id: str) -> Manifest:
    """A minimal launchable agent manifest."""
    return Manifest(
        role="agent",
        cls="noop_app:NoopAgent",
        subject_root="noopapp.episode.e1",
        agent_id=agent_id,
        nats_url=NATS_URL,
    )


class FakeMessage:
    """A pulled manifest message recording how the worker resolved it."""

    def __init__(self, manifest: Manifest) -> None:
        self.data = manifest.model_dump_json().encode()
        self.acked = False
        self.naked = False
        self.termed = False

    async def ack(self) -> None:
        self.acked = True

    async def nak(self) -> None:
        self.naked = True

    async def term(self) -> None:
        self.termed = True


class FakePullSubscription:
    """Serves pre-loaded batches of messages, then empty batches forever."""

    def __init__(self, batches: list[list[FakeMessage]]) -> None:
        self._batches = batches
        self.fetch_calls: list[int] = []

    async def fetch(self, batch: int, timeout: float) -> Sequence[FakeMessage]:  # noqa: ASYNC109
        self.fetch_calls.append(batch)
        if self._batches:
            return self._batches.pop(0)
        await asyncio.sleep(timeout)
        return []


class FakeTransport:
    """A transport stub exposing only the work-queue surface the worker uses."""

    def __init__(self, subscription: FakePullSubscription) -> None:
        self._subscription = subscription
        self.connected = False
        self.closed = False
        self.ensured_stream: tuple[str, tuple[str, ...]] | None = None
        self.ensured_consumer: tuple[str, str] | None = None

    async def connect(self) -> None:
        self.connected = True

    async def close(self) -> None:
        self.closed = True

    async def ensure_work_queue_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None:
        self.ensured_stream = (name, tuple(subjects))

    async def ensure_work_consumer(self, stream: str, durable: str, *, ack_wait: float) -> None:
        self.ensured_consumer = (stream, durable)

    async def pull_subscribe(
        self, stream: str, durable: str, subjects: Sequence[str]
    ) -> FakePullSubscription:
        return self._subscription


class FakeChild:
    """A stand-in for an ``asyncio.subprocess.Process`` the worker supervises.

    *exit_after* is how long (seconds) before the process "exits" with
    *returncode*; ``None`` means it stays alive until terminated. *ignore_term*
    models a straggler that survives ``terminate()`` and only dies on ``kill()``.
    """

    def __init__(
        self,
        *,
        exit_after: float | None,
        returncode: int = EXIT_ENDED,
        ignore_term: bool = False,
    ) -> None:
        self._exit_after = exit_after
        self._returncode = returncode
        self._ignore_term = ignore_term
        self.returncode: int | None = None
        self.terminated = False
        self.killed = False
        self._exited = asyncio.Event()
        if exit_after is not None:
            asyncio.get_running_loop().call_later(exit_after, self._exit)

    def _exit(self) -> None:
        if self.returncode is None:
            self.returncode = self._returncode
            self._exited.set()

    async def wait(self) -> int:
        await self._exited.wait()
        assert self.returncode is not None
        return self.returncode

    def terminate(self) -> None:
        self.terminated = True
        if not self._ignore_term:
            self.returncode = self._returncode
            self._exited.set()

    def kill(self) -> None:
        self.killed = True
        self.returncode = -9
        self._exited.set()


def _worker(
    children: list[FakeChild],
    *,
    transport: FakeTransport,
    concurrency: int = 4,
    readiness_window: float = 0.05,
) -> Worker:
    """A worker whose child factory yields *children* in order, FIFO."""
    queue = list(children)

    async def spawn(manifest: Manifest) -> FakeChild:
        return queue.pop(0)

    return Worker(
        transport,  # type: ignore[arg-type]
        concurrency=concurrency,
        readiness_window=readiness_window,
        spawn=spawn,  # type: ignore[arg-type]
    )


async def test_confirmed_child_is_acked_and_supervised() -> None:
    msg = FakeMessage(_manifest("alpha"))
    sub = FakePullSubscription([[msg]])
    transport = FakeTransport(sub)
    # A child that stays up through its readiness window, then ends cleanly.
    child = FakeChild(exit_after=0.3, returncode=EXIT_ENDED)
    worker = _worker([child], transport=transport)

    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(0.15)  # past the readiness window
    assert msg.acked is True
    assert msg.naked is False

    worker.stop()
    await runner
    assert transport.ensured_stream is not None
    assert transport.ensured_consumer is not None
    assert transport.closed is True


async def test_child_that_fails_to_start_is_naked_not_acked() -> None:
    msg = FakeMessage(_manifest("alpha"))
    sub = FakePullSubscription([[msg]])
    transport = FakeTransport(sub)
    # A child that dies inside its readiness window with the transport-failure code.
    child = FakeChild(exit_after=0.0, returncode=EXIT_TRANSPORT)
    worker = _worker([child], transport=transport)

    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(0.15)
    assert msg.acked is False
    assert msg.naked is True

    worker.stop()
    await runner


async def test_concurrency_cap_limits_fetch_batch() -> None:
    sub = FakePullSubscription([])  # never serves anything; we inspect fetch sizes
    transport = FakeTransport(sub)
    worker = _worker([], transport=transport, concurrency=3)

    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(0.05)
    worker.stop()
    await runner
    # With no live children, spare capacity is the full cap.
    assert sub.fetch_calls
    assert sub.fetch_calls[0] == 3


async def test_stop_hard_kills_a_straggler() -> None:
    msg = FakeMessage(_manifest("alpha"))
    sub = FakePullSubscription([[msg]])
    transport = FakeTransport(sub)
    # Comes up fine, but ignores terminate(): only kill() ends it.
    child = FakeChild(exit_after=None, ignore_term=True)
    worker = _worker([child], transport=transport)

    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(0.15)
    assert msg.acked is True

    worker.stop()
    await asyncio.wait_for(runner, timeout=10)
    assert child.terminated is True
    assert child.killed is True


async def test_run_is_idempotent_on_double_stop() -> None:
    sub = FakePullSubscription([])
    transport = FakeTransport(sub)
    worker = _worker([], transport=transport)
    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(0.02)
    worker.stop()
    worker.stop()  # second stop is a no-op, not an error
    await runner
    assert transport.closed is True


async def test_worker_acks_off_the_in_memory_work_queue() -> None:
    """Against the real MemoryTransport work queue: a confirmed child acks the manifest.

    Exercises the in-memory work-queue mirror end to end -- enqueue a manifest,
    the worker pulls it, its (fake) child comes up, and the worker acks, so the
    manifest is gone from the queue (work-queue retention).
    """
    transport = MemoryTransport()
    await transport.ensure_work_queue_stream(WORK_QUEUE_STREAM, list(WORK_QUEUE_SUBJECTS))
    manifest = _manifest("alpha")
    await transport.publish(work_subject("noopapp", "ep1"), manifest.model_dump_json().encode())

    child = FakeChild(exit_after=None)  # stays up

    async def spawn(_manifest: Manifest) -> FakeChild:
        return child

    worker = Worker(transport, readiness_window=0.05, spawn=spawn)  # type: ignore[arg-type]
    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(0.2)
    worker.stop()
    await asyncio.wait_for(runner, timeout=10)

    # The manifest was acked: nothing is left pending on the queue, and a fresh
    # bind sees an empty fetch.
    sub = await transport.pull_subscribe(
        WORK_QUEUE_STREAM, WORK_QUEUE_CONSUMER, WORK_QUEUE_SUBJECTS
    )
    leftover = await sub.fetch(10, 0.05)
    assert leftover == []


async def test_naked_manifest_is_redelivered_on_the_in_memory_queue() -> None:
    """A child that dies in its window naks; the manifest returns to the queue."""
    transport = MemoryTransport()
    await transport.ensure_work_queue_stream(WORK_QUEUE_STREAM, list(WORK_QUEUE_SUBJECTS))
    await transport.publish(
        work_subject("noopapp", "ep1"), _manifest("alpha").model_dump_json().encode()
    )

    # The child dies immediately with the transport-failure code: not acked.
    dying = FakeChild(exit_after=0.0, returncode=EXIT_TRANSPORT)

    async def spawn(_manifest: Manifest) -> FakeChild:
        return dying

    worker = Worker(transport, readiness_window=0.05, spawn=spawn)  # type: ignore[arg-type]
    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(0.2)
    worker.stop()
    await asyncio.wait_for(runner, timeout=10)

    # The manifest was naked, so it is back on the queue for another worker.
    sub = await transport.pull_subscribe(
        WORK_QUEUE_STREAM, WORK_QUEUE_CONSUMER, WORK_QUEUE_SUBJECTS
    )
    redelivered = await sub.fetch(10, 0.05)
    assert len(redelivered) == 1
