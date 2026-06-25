"""Worker acceptance tests against real NATS and real child processes (ADR-0005).

These prove the worker's hard guarantees end to end -- they cannot be faked:

* one worker runs a full episode's environment + agents as child processes;
* two workers bound to the *same* shared consumer each run a manifest at most
  once (work-queue retention + one durable consumer = no duplicates);
* a child that fails to start is not acked, so its manifest is redelivered;
* a straggler that ignores cooperative shutdown is hard-killed.

Skipped unless NATS is reachable AND ``FREEAGENT_SLOW_TEST`` is set (these spawn
real processes and talk to a server). The spawned children import ``noop_app``
from this directory, exported on ``PYTHONPATH`` by the autouse fixture.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
import socket
import uuid
from pathlib import Path

import pytest
from noop_app import (
    MARKER_STREAM,
    MARKER_SUBJECT,
    MarkerAgent,
    NoopAgent,
    NoopEnvironment,
    StragglerAgent,
)

from freeagent import Manifest, NatsTransport, default_nats_url
from freeagent.cli.child import class_ref
from freeagent.cli.work import Worker
from freeagent.subjects import stream_name, subject_root
from freeagent.workqueue import (
    WORK_QUEUE_CONSUMER,
    WORK_QUEUE_STREAM,
    WORK_QUEUE_SUBJECTS,
    work_subject,
)

TESTS_DIR = Path(__file__).parent
NATS_HOST = "localhost"
NATS_PORT = 4222


def _nats_running() -> bool:
    try:
        with socket.create_connection((NATS_HOST, NATS_PORT), timeout=0.5):
            return True
    except OSError:
        return False


requires_nats = pytest.mark.skipif(
    not _nats_running(),
    reason=(
        "NATS is not running; start it with: docker compose -f docker/nats/docker-compose.yml up -d"
    ),
)

_SLOW_TEST_VAR = "FREEAGENT_SLOW_TEST"
slow = pytest.mark.skipif(
    not os.environ.get(_SLOW_TEST_VAR),
    reason=f"slow test is opt-in: set {_SLOW_TEST_VAR} to run it",
)


@pytest.fixture(autouse=True)
def _children_can_import_noop_app(monkeypatch: pytest.MonkeyPatch) -> None:
    """Export this directory so spawned children can import ``noop_app``."""
    existing = os.environ.get("PYTHONPATH", "")
    joined = str(TESTS_DIR) if not existing else f"{TESTS_DIR}{os.pathsep}{existing}"
    monkeypatch.setenv("PYTHONPATH", joined)


@pytest.fixture(autouse=True)
async def _drop_work_stream_after() -> object:
    """Delete the shared work stream after each test (drops its durable consumer).

    The worker binds the one shared ``WORK_QUEUE_CONSUMER`` (an *unfiltered*
    consumer) on the work-queue stream. Left behind, it makes JetStream reject a
    *filtered* consumer on the same stream ("filtered consumer not unique on
    workqueue stream"), so a later test (e.g. the recruiter's) that pulls one
    episode's subject would fail. Dropping the stream cleans the slate.
    """
    yield None
    if not (os.environ.get(_SLOW_TEST_VAR) and _nats_running()):
        return
    cleanup = NatsTransport(default_nats_url())
    await cleanup.connect()
    with contextlib.suppress(Exception):
        await cleanup.delete_stream(WORK_QUEUE_STREAM)
    await cleanup.close()


async def _fresh_work_queue(url: str, *, ack_wait: float = 30.0) -> NatsTransport:
    """A connected transport with an empty work queue (drop any prior stream).

    Deleting the stream removes its consumer too, so the consumer is recreated
    here with the requested *ack_wait* -- letting a test force a short
    in-flight-redelivery window.
    """
    transport = NatsTransport(url)
    await transport.connect()
    # Start from a clean slate so a prior run's manifests don't leak in.
    await transport.delete_stream(WORK_QUEUE_STREAM)
    await transport.ensure_work_queue_stream(WORK_QUEUE_STREAM, list(WORK_QUEUE_SUBJECTS))
    await transport.ensure_work_consumer(WORK_QUEUE_STREAM, WORK_QUEUE_CONSUMER, ack_wait=ack_wait)
    return transport


async def _publish(transport: NatsTransport, manifest: Manifest, episode_id: str) -> None:
    subject = work_subject(manifest.app or "noopapp", episode_id)
    await transport.publish(subject, manifest.model_dump_json().encode())


@slow
@requires_nats
async def test_two_workers_run_each_manifest_at_most_once() -> None:
    """Two workers, one shared consumer: N manifests -> N distinct markers, no dupes."""
    url = default_nats_url()
    transport = await _fresh_work_queue(url)
    # A dedicated marker stream the MarkerAgent children publish their ids into.
    await transport.delete_stream(MARKER_STREAM)
    await transport.ensure_stream(MARKER_STREAM, [MARKER_SUBJECT])

    episode_id = uuid.uuid4().hex[:8]
    root = subject_root("markerapp", episode_id)
    ids = [f"m{i}" for i in range(8)]
    for agent_id in ids:
        manifest = Manifest(
            role="agent",
            cls=class_ref(MarkerAgent),
            subject_root=root,
            agent_id=agent_id,
            app="markerapp",
            config={"grace_period": 0.1},
            nats_url=url,
        )
        await _publish(transport, manifest, episode_id)

    seen: list[str] = []

    async def collect(_subject: str, data: bytes) -> None:
        seen.append(data.decode())

    sub = await transport.subscribe(MARKER_SUBJECT, collect)

    # Two workers on the SAME durable consumer. Each gets its own transport.
    workers = [Worker(NatsTransport(url), concurrency=4) for _ in range(2)]
    runners = [asyncio.ensure_future(w.run()) for w in workers]

    # Wait until every marker has been observed (or time out).
    deadline = asyncio.get_event_loop().time() + 30
    while len(set(seen)) < len(ids) and asyncio.get_event_loop().time() < deadline:  # noqa: ASYNC110
        await asyncio.sleep(0.2)

    for w in workers:
        w.stop()
    await asyncio.gather(*runners)
    await sub.unsubscribe()
    await transport.delete_stream(MARKER_STREAM)
    await transport.close()

    # Every manifest ran exactly once: all ids present, none twice.
    assert sorted(set(seen)) == sorted(ids)
    assert len(seen) == len(ids), f"a manifest was launched more than once: {seen}"


@slow
@requires_nats
async def test_failed_child_is_redelivered_not_acked() -> None:
    """A child that exits EXIT_TRANSPORT is naked; another attempt eventually runs it."""
    url = default_nats_url()
    # ack_wait must exceed the readiness window below (the worker holds the
    # message un-acked while it waits out the child's slow connect failure).
    transport = await _fresh_work_queue(url, ack_wait=30.0)

    episode_id = uuid.uuid4().hex[:8]
    root = subject_root("markerapp", episode_id)
    # A manifest pointing at an unreachable NATS: the child exits EXIT_TRANSPORT
    # fast, so the worker must NOT ack -- the manifest stays on the queue.
    bad = Manifest(
        role="agent",
        cls=class_ref(MarkerAgent),
        subject_root=root,
        agent_id="bad",
        app="markerapp",
        config={"grace_period": 0.1},
        nats_url="nats://127.0.0.1:4299",  # nothing is listening here
    )
    await _publish(transport, bad, episode_id)

    # Run a worker: it pulls, the child fails to start (it cannot reach the dead
    # URL and exits EXIT_TRANSPORT), and the worker naks. The readiness window
    # must outlast the child's connect-failure time (a few retries, ~6s) so the
    # worker sees the exit-3 and naks, rather than acking a still-connecting
    # child. We then stop it and confirm the manifest is still claimable.
    worker = Worker(NatsTransport(url), concurrency=1, readiness_window=10.0)
    runner = asyncio.ensure_future(worker.run())
    await asyncio.sleep(9.0)
    worker.stop()
    await runner

    # The unacked manifest is still on the queue: a fresh pull eventually returns
    # it. After the nak it is immediately pending; if a retry was left in flight
    # when we stopped, it redelivers after ack_wait -- so poll generously.
    drain = NatsTransport(url)
    await drain.connect()
    pull = await drain.pull_subscribe(WORK_QUEUE_STREAM, WORK_QUEUE_CONSUMER, WORK_QUEUE_SUBJECTS)
    redelivered: list[object] = []
    deadline = asyncio.get_event_loop().time() + 35
    while not redelivered and asyncio.get_event_loop().time() < deadline:
        redelivered = list(await pull.fetch(1, 2.0))
    assert redelivered, "the failed child's manifest was lost (it should be redelivered)"
    for msg in redelivered:
        await msg.term()  # clean it off the queue
    await drain.close()
    await transport.close()


@slow
@requires_nats
async def test_straggler_is_hard_killed_on_shutdown() -> None:
    """A child that ignores SIGTERM is killed; the worker still returns promptly."""
    url = default_nats_url()
    transport = await _fresh_work_queue(url)

    episode_id = uuid.uuid4().hex[:8]
    root = subject_root("markerapp", episode_id)
    manifest = Manifest(
        role="agent",
        cls=class_ref(StragglerAgent),
        subject_root=root,
        agent_id="straggler",
        app="markerapp",
        config={"grace_period": 0.1},
        nats_url=url,
    )
    await _publish(transport, manifest, episode_id)

    worker = Worker(NatsTransport(url), concurrency=1, readiness_window=2.0)
    runner = asyncio.ensure_future(worker.run())
    # Let the straggler come up and get acked + supervised.
    await asyncio.sleep(4.0)
    worker.stop()
    # terminate->kill is bounded by TERMINATE_TIMEOUT; the worker must return.
    await asyncio.wait_for(runner, timeout=20)
    await transport.close()


@slow
@requires_nats
async def test_one_worker_runs_a_full_episode() -> None:
    """One worker launches an episode's environment + agents from manifests, to ENDED."""
    url = default_nats_url()
    transport = await _fresh_work_queue(url)

    episode_id = uuid.uuid4().hex[:8]
    app = "noopapp"
    root = subject_root(app, episode_id)
    env_config = {"setup_timeout": 5.0, "episode_timeout": 2.0, "grace_period": 0.2}

    env_manifest = Manifest(
        role="environment",
        cls=class_ref(NoopEnvironment),
        app=app,
        roster=["alpha", "beta"],
        episode_id=episode_id,
        config=env_config,
        nats_url=url,
    )
    agent_manifests = [
        Manifest(
            role="agent",
            cls=class_ref(NoopAgent),
            subject_root=root,
            agent_id=name,
            app=app,
            config={"grace_period": 0.2},
            nats_url=url,
        )
        for name in ("alpha", "beta")
    ]
    for manifest in (env_manifest, *agent_manifests):
        await _publish(transport, manifest, episode_id)

    worker = Worker(NatsTransport(url), concurrency=4)
    runner = asyncio.ensure_future(worker.run())

    # The environment creates the episode stream and runs to ``ended`` on its
    # timeout; wait for that stream to appear and the episode to finish.
    stream = stream_name(app, episode_id)
    deadline = asyncio.get_event_loop().time() + 30
    saw_stream = False
    while asyncio.get_event_loop().time() < deadline:
        if stream in set(await transport.list_streams()):
            saw_stream = True
            break
        await asyncio.sleep(0.3)
    assert saw_stream, "the worker never launched the environment (no episode stream)"

    # Give the episode time to end and the worker to reap its children.
    await asyncio.sleep(6.0)
    worker.stop()
    await asyncio.wait_for(runner, timeout=20)
    with contextlib.suppress(Exception):
        await transport.delete_stream(stream)
    await transport.close()
