"""The worker: ``free-agent work``, a manifest-pulling episode supervisor (ADR-0005).

A long-lived, app-agnostic process that pulls :class:`~freeagent.Manifest`
messages off the shared JetStream work queue and runs each as a child process.
It is the orchestrator's launch step (``child.py``) wrapped in a pull loop plus
the terminate->kill straggler escalation the orchestrator already uses
(:mod:`freeagent.cli.supervise`).

The hard part is *ack semantics*. A worker acks a manifest on **confirmed
creation** -- the child survived its connect phase -- not on pull and not on the
role's completion. ``child.py`` exits fast with :data:`EXIT_TRANSPORT` when it
cannot reach NATS, so "still alive after a short readiness window" is the
readiness proxy: if the child has not exited within that window it is up, so we
ack and supervise it; if it died (transport failure or any early exit) we
``nak`` so the manifest is redelivered to another worker. Because the work
queue has work-queue retention and every worker binds the *one* shared durable
consumer, an acked manifest leaves the stream and reaches exactly one worker --
two workers each run a manifest at most once.

The worker keeps no state across restarts. A per-worker concurrency cap bounds
how many children it supervises at once; it never fetches more than its spare
capacity, so a manifest it cannot immediately run stays on the queue for a
worker that can. ``SIGTERM``/``SIGINT`` stop the pull loop and wind every live
child down cooperatively, then forcibly.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
import os
import signal
from typing import TYPE_CHECKING, Annotated, Protocol

import typer

from freeagent.manifest import Manifest
from freeagent.workqueue import WORK_QUEUE_CONSUMER, WORK_QUEUE_STREAM, WORK_QUEUE_SUBJECTS

from .child import EXIT_TRANSPORT
from .config import default_nats_url
from .supervise import spawn_child, terminate_all

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable, Iterator, Sequence

    from freeagent.transport import PulledMessage, PullSubscription

logger = logging.getLogger("freeagent.work")

#: Default children one worker supervises at once. Conservative: a worker pulls
#: only its spare capacity, so several small workers share the queue evenly.
DEFAULT_CONCURRENCY = 4

#: Seconds a freshly spawned child gets to survive its connect phase before the
#: worker calls it "up". ``child.py`` exits fast (:data:`EXIT_TRANSPORT`) when it
#: cannot reach NATS, so a child still alive after this window is the readiness
#: proxy. Generous enough to ride out import + connect on a loaded box.
DEFAULT_READINESS_WINDOW = 2.0

#: ``ack_wait`` on the shared consumer: it covers *startup only* (the worker acks
#: on confirmed creation), so a child that dies before its ack is redelivered.
ACK_WAIT = 30.0

#: Seconds a fetch waits for a manifest before returning empty so the loop can
#: re-poll (and notice a stop request).
PULL_TIMEOUT = 1.0


class WorkQueueTransport(Protocol):
    """The slice of the transport the worker needs: connect + the work queue.

    :class:`~freeagent.NatsTransport` and :class:`~freeagent.MemoryTransport`
    both satisfy it; tests can also supply a narrow fake.
    """

    async def connect(self) -> None: ...
    async def close(self) -> None: ...
    async def ensure_work_queue_stream(
        self, name: str, subjects: Sequence[str], *, metadata: dict[str, str] | None = None
    ) -> None: ...
    async def ensure_work_consumer(self, stream: str, durable: str, *, ack_wait: float) -> None: ...
    async def pull_subscribe(
        self, stream: str, durable: str, subjects: Sequence[str]
    ) -> PullSubscription: ...


if TYPE_CHECKING:
    #: Spawns the child process a manifest describes; injectable for unit tests.
    SpawnChild = Callable[[Manifest], Awaitable[asyncio.subprocess.Process]]


async def _default_spawn(manifest: Manifest) -> asyncio.subprocess.Process:
    """Launch the child the orchestrator would, from the manifest's child spec."""
    return await spawn_child(manifest.to_child_spec(), dict(os.environ))


class Worker:
    """Pulls manifests off the work queue and supervises their child processes.

    Construct with a :class:`WorkQueueTransport` (a :class:`NatsTransport` in
    production), then :meth:`run` the pull loop. :meth:`stop` ends the loop and
    winds every live child down. The *spawn* hook is the child factory; it
    defaults to launching ``child.py`` and is overridden in unit tests.
    """

    def __init__(
        self,
        transport: WorkQueueTransport,
        *,
        concurrency: int = DEFAULT_CONCURRENCY,
        readiness_window: float = DEFAULT_READINESS_WINDOW,
        spawn: SpawnChild | None = None,
    ) -> None:
        self._transport = transport
        self._concurrency = concurrency
        self._readiness_window = readiness_window
        self._spawn = spawn if spawn is not None else _default_spawn
        self._stop = asyncio.Event()
        #: Live children we have acked and are supervising, each to a reaper task.
        self._children: dict[asyncio.subprocess.Process, asyncio.Task[None]] = {}

    def stop(self) -> None:
        """Stop pulling and trigger shutdown of every live child (idempotent)."""
        self._stop.set()

    async def run(self) -> None:
        """Connect, bind the shared consumer, and pull-launch-ack until stopped.

        Returns once :meth:`stop` is called and every live child has been wound
        down (cooperatively, then forcibly). The connection is always closed.
        """
        await self._transport.connect()
        try:
            await self._transport.ensure_work_queue_stream(
                WORK_QUEUE_STREAM, list(WORK_QUEUE_SUBJECTS)
            )
            await self._transport.ensure_work_consumer(
                WORK_QUEUE_STREAM, WORK_QUEUE_CONSUMER, ack_wait=ACK_WAIT
            )
            subscription = await self._transport.pull_subscribe(
                WORK_QUEUE_STREAM, WORK_QUEUE_CONSUMER, WORK_QUEUE_SUBJECTS
            )
            await self._pull_loop(subscription)
        finally:
            await self._shutdown_children()
            await self._transport.close()

    async def _pull_loop(self, subscription: PullSubscription) -> None:
        """Fetch spare-capacity batches and dispatch each manifest, until stopped."""
        while not self._stop.is_set():
            spare = self._concurrency - len(self._children)
            if spare <= 0:
                # At capacity: wait for a child to exit (or a stop) before pulling.
                await self._wait_for_capacity()
                continue
            messages = await subscription.fetch(spare, PULL_TIMEOUT)
            for message in messages:
                if self._stop.is_set():
                    # Stopping mid-batch: don't claim work we won't supervise.
                    await message.nak()
                    continue
                await self._handle_message(message)

    async def _handle_message(self, message: PulledMessage) -> None:
        """Launch the manifest's child; ack if it comes up, nak if startup failed.

        "Came up" means the child took ownership of the work: it either survived
        its readiness window (still running) or exited for a reason *other* than
        a transport failure (it reached NATS and ran -- a fast, legitimately
        short role is up, not failed). Only :data:`EXIT_TRANSPORT` within the
        window is a startup failure: the child never reached NATS, so the
        manifest is naked and redelivered to a worker that can. Acking a child
        that already ran -- or naking one that did -- would double-run the role.
        """
        manifest = Manifest.model_validate_json(message.data)
        child = await self._spawn(manifest)
        if await self._failed_to_start(child):
            await message.nak()  # never reached NATS: redeliver, do NOT ack
            return
        await message.ack()
        self._supervise(child)

    async def _failed_to_start(self, child: asyncio.subprocess.Process) -> bool:
        """True iff *child* exited with :data:`EXIT_TRANSPORT` inside its window.

        Still running after the window, or exited with any other code, counts as
        "came up": the child reached its connect phase and took the work.
        """
        waiter = asyncio.ensure_future(child.wait())
        try:
            exit_code = await asyncio.wait_for(asyncio.shield(waiter), self._readiness_window)
        except TimeoutError:
            return False  # still running past the window: confirmed up
        else:
            return exit_code == EXIT_TRANSPORT
        finally:
            if not waiter.done():
                waiter.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await waiter

    def _supervise(self, child: asyncio.subprocess.Process) -> None:
        """Track *child* as live and reap it (drop from the set) when it exits."""
        self._children[child] = asyncio.ensure_future(self._reap(child))

    async def _reap(self, child: asyncio.subprocess.Process) -> None:
        """Await the child's exit, then drop it from the live set."""
        with contextlib.suppress(asyncio.CancelledError):
            await child.wait()
        self._children.pop(child, None)

    async def _wait_for_capacity(self) -> None:
        """Block until a supervised child exits or a stop is requested."""
        if not self._children:
            return
        reapers = list(self._children.values())
        stop = asyncio.ensure_future(self._stop.wait())
        try:
            await asyncio.wait({*reapers, stop}, return_when=asyncio.FIRST_COMPLETED)
        finally:
            if not stop.done():
                stop.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await stop

    async def _shutdown_children(self) -> None:
        """Cancel reapers and wind every live child down: terminate, then kill."""
        children = list(self._children)
        for reaper in self._children.values():
            reaper.cancel()
        for reaper in list(self._children.values()):
            with contextlib.suppress(asyncio.CancelledError):
                await reaper
        self._children.clear()
        await terminate_all(children)


async def work(
    transport: WorkQueueTransport,
    *,
    concurrency: int = DEFAULT_CONCURRENCY,
    install_signals: bool = False,
) -> None:
    """Run one worker against *transport* until stopped.

    When *install_signals* is set (the CLI path), SIGINT/SIGTERM stop the worker
    and enforce local shutdown on its children; an in-process caller passes
    ``False`` and drives :meth:`Worker.stop` itself.
    """
    worker = Worker(transport, concurrency=concurrency)
    if install_signals:
        with _install_signal_handlers(worker):
            await worker.run()
    else:
        await worker.run()


@contextlib.contextmanager
def _install_signal_handlers(worker: Worker) -> Iterator[None]:
    """Route SIGINT/SIGTERM to ``worker.stop`` for the duration (CLI only)."""
    loop = asyncio.get_running_loop()
    handled: list[signal.Signals] = []
    for sig in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(NotImplementedError, RuntimeError):
            loop.add_signal_handler(sig, worker.stop)
            handled.append(sig)
    try:
        yield
    finally:
        for sig in handled:
            loop.remove_signal_handler(sig)


def _work_command(
    nats_url: Annotated[
        str | None,
        typer.Option(
            "--nats-url",
            help="NATS server URL to pull manifests from; "
            "defaults to $FREEAGENT_NATS_URL or nats://localhost:4222.",
        ),
    ] = None,
    concurrency: Annotated[
        int,
        typer.Option(
            "--concurrency",
            min=1,
            help="how many episode children this worker supervises at once.",
        ),
    ] = DEFAULT_CONCURRENCY,
) -> None:
    """Run a FreeAgent worker: pull episode manifests off the work queue and run them.

    A long-lived, app-agnostic supervisor. It binds the one shared durable pull
    consumer, so two workers each run a manifest at most once; stop it with
    Ctrl-C (SIGTERM), which winds its children down cooperatively, then forcibly.
    """
    # NatsTransport pulls in nats-py; import it lazily so the rest of the CLI need
    # not load the transport stack just to render help.
    from freeagent.transport import NatsTransport

    url = nats_url if nats_url is not None else default_nats_url()
    transport = NatsTransport(url)
    asyncio.run(work(transport, concurrency=concurrency, install_signals=True))
    typer.echo("free-agent: worker stopped", err=True)
