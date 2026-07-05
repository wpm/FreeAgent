"""Unit tests for :mod:`freeagent.api.episodes`: the monitor's state machine and the manager.

The monitor is pure bookkeeping, so the whole control/data plane split (ADR-0007) is exercised
here message by message without a server; the manager runs against the fakes in
``api_fakes.py``. The
over-the-wire behavior is covered by ``test_integration.py``.
"""

from __future__ import annotations

import json
import sys
from typing import cast

import pytest
from api_fakes import FakeMsg, FakeNatsClient, FakeProcess
from freeagent.api.episodes import (
    WORKER_MODULE,
    DuplicateEpisode,
    Episode,
    EpisodeManager,
    EpisodeMonitor,
    EpisodeState,
    NatsClient,
    UnknownEpisode,
    WorkerProcess,
)
from freeagent.app.collatz.message import Chain
from freeagent.sdk import UnknownApplication
from freeagent.sdk.message import Ack, EpisodeComplete, StartEntity, StopAgent, StopEntity
from nats.aio.msg import Msg

ROOT = "episode.collatz.test"


def make_monitor() -> EpisodeMonitor:
    return EpisodeMonitor("collatz", "test", ROOT)


def test_fresh_monitor_is_created_and_empty() -> None:
    monitor = make_monitor()
    assert monitor.state is EpisodeState.CREATED
    assert monitor.agents_alive == set()
    assert monitor.messages == []


def test_start_entity_on_an_agent_subject_marks_the_agent_alive() -> None:
    monitor = make_monitor()
    monitor.record(f"{ROOT}.agents.agent-0", StartEntity().to_bytes(), 1.0)
    assert monitor.state is EpisodeState.RUNNING
    assert monitor.agents_alive == {"agent-0"}
    assert monitor.messages == []


def test_stop_agent_and_stop_entity_each_mark_the_agent_stopped() -> None:
    monitor = make_monitor()
    monitor.record(f"{ROOT}.agents.a", StartEntity().to_bytes(), 1.0)
    monitor.record(f"{ROOT}.agents.b", StartEntity().to_bytes(), 2.0)
    monitor.record(f"{ROOT}.agents.a", StopAgent().to_bytes(), 3.0)
    assert monitor.agents_alive == {"b"}
    monitor.record(f"{ROOT}.agents.b", StopEntity().to_bytes(), 4.0)
    assert monitor.agents_alive == set()


def test_control_commands_off_the_agent_subtree_change_no_agent_state() -> None:
    monitor = make_monitor()
    monitor.record(f"{ROOT}.environment", StartEntity().to_bytes(), 1.0)
    monitor.record(f"{ROOT}.agents.a.extra", StartEntity().to_bytes(), 2.0)
    monitor.record(f"{ROOT}.agents", StartEntity().to_bytes(), 3.0)
    assert monitor.agents_alive == set()
    assert monitor.state is EpisodeState.RUNNING
    assert monitor.messages == []


def test_episode_complete_is_terminal() -> None:
    monitor = make_monitor()
    monitor.record(f"{ROOT}.environment", EpisodeComplete().to_bytes(), 1.0)
    assert monitor.state is EpisodeState.COMPLETE
    # Trailing traffic (the StopEntity broadcast follows the end marker) doesn't resurrect it.
    monitor.record(f"{ROOT}.agents.a", StopEntity().to_bytes(), 2.0)
    monitor.mark_stopped()
    monitor.mark_failed()
    assert monitor.state is EpisodeState.COMPLETE


def test_ack_is_control_plane_and_leaves_no_trace() -> None:
    monitor = make_monitor()
    monitor.record(f"{ROOT}.environment", Ack().to_bytes(), 1.0)
    assert monitor.state is EpisodeState.RUNNING
    assert monitor.messages == []


def test_app_defined_message_lands_in_the_data_plane_feed() -> None:
    monitor = make_monitor()
    chain = Chain(numbers=[8, 4])
    monitor.record(f"{ROOT}.agents.agent-0", chain.to_bytes(), 1.5)
    assert monitor.state is EpisodeState.RUNNING
    assert monitor.agents_alive == set()
    (record,) = monitor.messages
    assert record.seq == 0
    assert record.subject == f"{ROOT}.agents.agent-0"
    assert record.message_type == "Chain"
    assert record.received_at == 1.5
    assert record.payload == json.loads(chain.to_bytes())


def test_registered_non_control_sdk_types_still_land_in_the_feed() -> None:
    # Chain is *registered* in this process (the test imports it), so it decodes -- but the plane
    # split is by explicit control-plane membership, not by "does it decode" (ADR-0007). This
    # pins the regression where in-process app imports would silently drain the feed.
    monitor = make_monitor()
    monitor.record(f"{ROOT}.environment.replies.agent-0", Chain(numbers=[8]).to_bytes(), 1.0)
    assert [record.message_type for record in monitor.messages] == ["Chain"]


def test_unknown_type_tags_and_untagged_json_are_served_opaquely() -> None:
    monitor = make_monitor()
    monitor.record(f"{ROOT}.x", b'{"message_type": "NoSuchType", "value": 3}', 1.0)
    monitor.record(f"{ROOT}.x", b'{"value": 3}', 2.0)
    monitor.record(f"{ROOT}.x", b'{"message_type": 7}', 3.0)
    monitor.record(f"{ROOT}.x", b"[1, 2, 3]", 4.0)
    assert [record.message_type for record in monitor.messages] == ["NoSuchType", None, None, None]
    assert [record.seq for record in monitor.messages] == [0, 1, 2, 3]
    assert monitor.messages[3].payload == [1, 2, 3]


def test_malformed_control_payload_is_not_an_error() -> None:
    # A payload whose tag names a control type but whose fields don't validate is "not a
    # control-plane message", served opaquely rather than raised or dropped.
    monitor = make_monitor()
    monitor.record(f"{ROOT}.agents.a", b'{"message_type": "StartEntity", "protocol": 5}', 1.0)
    assert monitor.agents_alive == set()
    assert [record.message_type for record in monitor.messages] == ["StartEntity"]


def test_non_json_payloads_are_skipped_entirely() -> None:
    monitor = make_monitor()
    monitor.record(f"{ROOT}.x", b"\xff\xfe not json", 1.0)
    assert monitor.state is EpisodeState.CREATED
    assert monitor.messages == []


def test_mark_stopped_and_mark_failed_respect_terminal_states() -> None:
    monitor = make_monitor()
    monitor.mark_failed()
    assert monitor.state is EpisodeState.FAILED
    monitor.mark_stopped()
    assert monitor.state is EpisodeState.FAILED

    stopped = make_monitor()
    stopped.mark_stopped()
    assert stopped.state is EpisodeState.STOPPED
    stopped.mark_failed()
    assert stopped.state is EpisodeState.STOPPED


class ManagerHarness:
    """An :class:`EpisodeManager` wired to fakes, with the fakes kept inspectable."""

    def __init__(self, spawn_result: FakeProcess | None = None) -> None:
        self.client = FakeNatsClient()
        self.connect_calls = 0
        self.spawned: list[list[str]] = []
        self.process = spawn_result if spawn_result is not None else FakeProcess()

        async def connect(url: str) -> NatsClient:
            self.connect_calls += 1
            return self.client

        def spawn(command: list[str]) -> WorkerProcess:
            self.spawned.append(command)
            return self.process

        self.manager = EpisodeManager("nats://fake:4222", connect=connect, spawn=spawn)


async def test_create_subscribes_to_the_episode_root_and_spawns_a_worker() -> None:
    harness = ManagerHarness()
    episode = await harness.manager.create("collatz", "ep1", {"starts": [8]})

    (subscription,) = harness.client.subscriptions
    assert subscription.subject == "episode.collatz.ep1.>"
    assert harness.spawned == [
        [
            sys.executable,
            "-m",
            WORKER_MODULE,
            "run",
            "collatz",
            "--episode-id",
            "ep1",
            "--episode-root",
            "episode.collatz.ep1",
            "--nats-url",
            "nats://fake:4222",
            "--config",
            json.dumps({"starts": [8]}),
        ]
    ]
    status = episode.status()
    assert status.state is EpisodeState.CREATED
    assert status.application == "collatz"
    assert status.episode_id == "ep1"
    assert status.episode_root == "episode.collatz.ep1"
    assert status.agents_alive == []
    assert status.message_count == 0
    assert status.worker_exit_code is None


async def test_create_generates_a_subject_safe_episode_id_when_none_is_given() -> None:
    harness = ManagerHarness()
    first = await harness.manager.create("collatz", None, {})
    second = await harness.manager.create("collatz", None, {})
    for episode in (first, second):
        assert episode.monitor.episode_id.isalnum()
    assert first.monitor.episode_id != second.monitor.episode_id
    # One shared NATS connection serves both episodes.
    assert harness.connect_calls == 1


async def test_create_rejects_unknown_applications_and_bad_names() -> None:
    harness = ManagerHarness()
    with pytest.raises(UnknownApplication):
        await harness.manager.create("no-such-app", None, {})
    # A name that can't be a subject token can't name an installed application either.
    with pytest.raises(UnknownApplication):
        await harness.manager.create("bad.name", None, {})
    assert harness.client.subscriptions == []
    assert harness.spawned == []


async def test_create_rejects_episode_ids_that_are_not_subject_tokens() -> None:
    harness = ManagerHarness()
    with pytest.raises(ValueError, match="subject token"):
        await harness.manager.create("collatz", "bad.id", {})
    assert harness.client.subscriptions == []


async def test_create_rejects_duplicate_episodes() -> None:
    harness = ManagerHarness()
    await harness.manager.create("collatz", "ep1", {})
    with pytest.raises(DuplicateEpisode):
        await harness.manager.create("collatz", "ep1", {})


async def test_a_failed_spawn_leaves_no_subscription_behind() -> None:
    client = FakeNatsClient()

    async def connect(url: str) -> NatsClient:
        return client

    def spawn(command: list[str]) -> WorkerProcess:
        raise FileNotFoundError("no worker installed")

    manager = EpisodeManager("nats://fake:4222", connect=connect, spawn=spawn)
    with pytest.raises(FileNotFoundError):
        await manager.create("collatz", "ep1", {})
    (subscription,) = client.subscriptions
    assert subscription.unsubscribe_calls == 1
    with pytest.raises(UnknownEpisode):
        manager.get("collatz", "ep1")


async def test_the_subscription_callback_feeds_the_monitor() -> None:
    harness = ManagerHarness()
    episode = await harness.manager.create("collatz", "ep1", {})
    (subscription,) = harness.client.subscriptions
    msg = FakeMsg("episode.collatz.ep1.agents.agent-0", StartEntity().to_bytes())
    await subscription.cb(cast(Msg, msg))
    assert episode.monitor.state is EpisodeState.RUNNING
    assert episode.monitor.agents_alive == {"agent-0"}


async def test_get_notices_a_dead_worker() -> None:
    harness = ManagerHarness()
    await harness.manager.create("collatz", "ep1", {})
    harness.process.returncode = 3
    episode = harness.manager.get("collatz", "ep1")
    assert episode.monitor.state is EpisodeState.FAILED
    assert episode.status().worker_exit_code == 3


async def test_get_treats_a_clean_worker_exit_as_no_signal() -> None:
    # Completion is judged from the wire's EpisodeComplete, never from the process exiting 0.
    harness = ManagerHarness()
    await harness.manager.create("collatz", "ep1", {})
    harness.process.returncode = 0
    episode = harness.manager.get("collatz", "ep1")
    assert episode.monitor.state is EpisodeState.CREATED


async def test_get_raises_for_unknown_episodes() -> None:
    harness = ManagerHarness()
    with pytest.raises(UnknownEpisode):
        harness.manager.get("collatz", "nope")


async def test_stop_terminates_the_worker_and_unsubscribes() -> None:
    harness = ManagerHarness()
    await harness.manager.create("collatz", "ep1", {})
    episode = await harness.manager.stop("collatz", "ep1")

    assert harness.process.terminate_calls == 1
    (subscription,) = harness.client.subscriptions
    assert subscription.unsubscribe_calls == 1
    assert episode.subscription is None
    assert episode.monitor.state is EpisodeState.STOPPED
    # A worker killed by the stop's terminate signal is a deliberate stop, not a failure.
    assert harness.manager.get("collatz", "ep1").monitor.state is EpisodeState.STOPPED


async def test_stop_is_idempotent_and_does_not_re_signal_an_exited_worker() -> None:
    harness = ManagerHarness()
    await harness.manager.create("collatz", "ep1", {})
    harness.process.returncode = 0
    await harness.manager.stop("collatz", "ep1")
    await harness.manager.stop("collatz", "ep1")
    assert harness.process.terminate_calls == 0
    (subscription,) = harness.client.subscriptions
    assert subscription.unsubscribe_calls == 1


async def test_stop_escalates_to_kill_when_terminate_is_ignored() -> None:
    harness = ManagerHarness(spawn_result=FakeProcess(stubborn=True))
    await harness.manager.create("collatz", "ep1", {})
    await harness.manager.stop("collatz", "ep1")
    assert harness.process.terminate_calls == 1
    assert harness.process.kill_calls == 1


async def test_stop_keeps_a_completed_state() -> None:
    harness = ManagerHarness()
    episode = await harness.manager.create("collatz", "ep1", {})
    episode.monitor.record("episode.collatz.ep1.environment", EpisodeComplete().to_bytes(), 1.0)
    await harness.manager.stop("collatz", "ep1")
    assert episode.monitor.state is EpisodeState.COMPLETE


async def test_shutdown_halts_every_episode_and_closes_the_client() -> None:
    harness = ManagerHarness()
    await harness.manager.create("collatz", "ep1", {})
    await harness.manager.create("collatz", "ep2", {})
    await harness.manager.shutdown()
    assert harness.process.terminate_calls >= 1
    assert all(s.unsubscribe_calls == 1 for s in harness.client.subscriptions)
    assert harness.client.close_calls == 1


async def test_shutdown_without_a_connection_is_a_no_op() -> None:
    harness = ManagerHarness()
    await harness.manager.shutdown()
    assert harness.client.close_calls == 0


def test_status_reports_no_exit_code_without_a_process() -> None:
    monitor = make_monitor()
    episode = Episode(monitor=monitor, subscription=None, process=None)
    assert episode.status().worker_exit_code is None
