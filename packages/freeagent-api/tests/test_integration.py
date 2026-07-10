"""Integration tests: the API launches, watches, serves, and stops episodes over a real server.

This is rewrite step 3's over-the-wire proof (issue #82). The API app is driven through HTTP (via
httpx's ASGI transport), real ``freeagent-worker`` subprocesses run real Collatz episodes against
a real ``nats-server``, and the acceptance criteria are asserted end to end:

- a Collatz episode created over REST runs to completion, and the message feed serves every
  ``Chain`` payload verbatim — byte-for-byte what the engine put on the wire, as recorded by an
  independent wire tap;
- episode status is derived from control-plane traffic (``StartEntity``/``StopAgent``/
  ``EpisodeComplete``), pinned deterministically by publishing those messages directly;
- two concurrent episodes of the same application do not mix feeds;
- ``DELETE`` stops a running episode and its worker.

Each test builds its own app so its manager (and NATS connection) is torn down with certainty in
its ``finally``; the ASGI transport skips lifespan, so teardown calls the manager directly.
"""

from __future__ import annotations

import asyncio
import json
import time
from typing import Any

import httpx2 as httpx
import nats
import pytest
from api_fakes import FakeProcess
from freeagent.api.app import create_app
from freeagent.api.episodes import EpisodeManager, WorkerProcess
from freeagent.sdk.message import EpisodeComplete, StartEntity, StopAgent
from nats.aio.msg import Msg

pytestmark = pytest.mark.integration

# Three starts of visibly different chain length (8 takes three steps; 27 takes 111), so agents
# finish at different times and the episode genuinely runs concurrently.
STARTS = [8, 6, 27]

POLL_INTERVAL = 0.02
"""Seconds between REST polls while waiting for an episode to change state."""

EPISODE_TIMEOUT = 60.0
"""Seconds before giving up on an episode reaching the awaited state."""


class WireTap:
    """A passive NATS connection recording every JSON payload on an episode's subtree.

    The independent record of "what the engine sent" that the feed endpoint is compared against: it
    subscribes to ``{episode_root}.>`` on its own connection, so what it sees is exactly what
    crossed the wire, unmediated by the API.
    """

    def __init__(self) -> None:
        self.frames: list[tuple[str, Any]] = []

    async def start(self, servers: str, episode_root: str) -> None:
        """Connect and subscribe to the episode's whole subject subtree.

        :param servers: The NATS server URL to connect to.
        :param episode_root: The episode root whose subtree (``{episode_root}.>``) to tap.
        """
        self._client = await nats.connect(servers)
        await self._client.subscribe(f"{episode_root}.>", cb=self._record)

    async def _record(self, msg: Msg) -> None:
        try:
            payload = json.loads(msg.data)
        except ValueError:
            return
        self.frames.append((msg.subject, payload))

    async def stop(self) -> None:
        """Disconnect the tap's own NATS connection."""
        await self._client.close()

    def chains_by_subject(self) -> dict[str, list[Any]]:
        """Group the tapped ``Chain`` payloads by the subject they crossed on, in wire order.

        :return: Each subject that carried a ``Chain``, mapped to its payloads in arrival order.
        """
        chains: dict[str, list[Any]] = {}
        for subject, payload in self.frames:
            if isinstance(payload, dict) and payload.get("message_type") == "Chain":
                chains.setdefault(subject, []).append(payload)
        return chains


def api_client(app: Any) -> httpx.AsyncClient:
    """Build an httpx client that drives ``app`` in-process over ASGI.

    :param app: The FastAPI app under test.
    :return: The client; requests to it never touch a network socket.
    """
    return httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://api")


async def poll_until(
    http: httpx.AsyncClient,
    path: str,
    done: Any,
    *,
    timeout: float = EPISODE_TIMEOUT,
) -> dict[str, Any]:
    """Poll an episode status endpoint until ``done(body)`` is true, failing fast on ``failed``.

    :param http: The client to poll with.
    :param path: The episode status path to poll.
    :param done: Predicate over the decoded status body.
    :param timeout: Seconds before the poll gives up and fails the test.
    :return: The first status body satisfying ``done``.
    """
    deadline = time.monotonic() + timeout
    while True:
        response = await http.get(path)
        assert response.status_code == 200, response.text
        body: dict[str, Any] = response.json()
        if done(body):
            return body
        assert body["state"] != "failed", f"episode failed while waiting: {body}"
        assert time.monotonic() < deadline, f"timed out waiting on {path}: {body}"
        await asyncio.sleep(POLL_INTERVAL)


async def test_collatz_episode_runs_to_completion_and_serves_its_feed_verbatim(
    nats_server: str,
) -> None:
    """A Collatz episode created via REST completes, and the feed matches the wire exactly.

    The tap subscribes to the (deterministic) episode root before the POST so it cannot miss a
    frame. After the status polls to ``complete``, every feed record must be a ``Chain`` (the
    control plane is filtered out) and, subject by subject, the feed's payloads must equal what the
    tap saw the engine send, in the same order.
    """
    app = create_app(nats_url=nats_server)
    manager: EpisodeManager = app.state.manager
    tap = WireTap()
    await tap.start(nats_server, "episode.collatz.feed")
    try:
        async with api_client(app) as http:
            listing = await http.get("/applications")
            assert listing.status_code == 200
            assert "collatz" in listing.json()["applications"]

            created = await http.post(
                "/applications/collatz/episodes",
                json={"episode_id": "feed", "config": {"starts": STARTS}},
            )
            assert created.status_code == 201, created.text
            assert created.json()["episode_root"] == "episode.collatz.feed"
            assert created.json()["state"] == "created"

            status = await poll_until(
                http,
                "/applications/collatz/episodes/feed",
                lambda body: body["state"] == "complete",
            )
            # EpisodeComplete arrived, and every started agent was stopped along the way.
            assert status["agents_alive"] == []

            # Let the tap's separate connection drain any trailing frames still in flight.
            await asyncio.sleep(0.2)
            feed = (await http.get("/applications/collatz/episodes/feed/messages")).json()[
                "messages"
            ]
    finally:
        await manager.shutdown()
        await tap.stop()

    # The feed is exactly the data plane: all Chains, ordered, none of the control plane.
    assert feed
    assert all(record["message_type"] == "Chain" for record in feed)
    assert [record["seq"] for record in feed] == list(range(len(feed)))

    # Verbatim pass-through: per subject, the feed's payloads are what the engine sent, in order.
    feed_chains: dict[str, list[Any]] = {}
    for record in feed:
        feed_chains.setdefault(record["subject"], []).append(record["payload"])
    assert feed_chains == tap.chains_by_subject()


async def test_status_reflects_start_stop_and_complete_control_messages(
    nats_server: str,
) -> None:
    """Control-plane traffic drives the status endpoint, pinned without a worker.

    The worker is faked out (nothing runs), and the SDK's own control messages are published
    straight onto the episode's subjects: ``StartEntity`` marks agents alive, ``StopAgent`` stops
    one, and ``EpisodeComplete`` ends the episode — each visible through REST as it lands.
    """

    def spawn(command: list[str]) -> WorkerProcess:
        return FakeProcess()

    manager = EpisodeManager(nats_server, spawn=spawn)
    app = create_app(manager=manager)
    publisher = await nats.connect(nats_server)
    root = "episode.collatz.wire"
    path = "/applications/collatz/episodes/wire"
    try:
        async with api_client(app) as http:
            created = await http.post("/applications/collatz/episodes", json={"episode_id": "wire"})
            assert created.status_code == 201, created.text
            assert created.json()["state"] == "created"

            await publisher.publish(f"{root}.agents.alpha", StartEntity().to_bytes())
            await publisher.publish(f"{root}.agents.beta", StartEntity().to_bytes())
            status = await poll_until(
                http, path, lambda body: body["agents_alive"] == ["alpha", "beta"]
            )
            assert status["state"] == "running"

            await publisher.publish(f"{root}.agents.alpha", StopAgent().to_bytes())
            await poll_until(http, path, lambda body: body["agents_alive"] == ["beta"])

            await publisher.publish(f"{root}.environment", EpisodeComplete().to_bytes())
            await poll_until(http, path, lambda body: body["state"] == "complete")
    finally:
        await manager.shutdown()
        await publisher.close()


async def test_concurrent_episodes_of_one_application_do_not_mix_feeds(
    nats_server: str,
) -> None:
    """Two episodes of the same app run at once; each feed carries only its own episode's chains.

    The episodes use disjoint starting numbers, so every Chain payload names which episode it
    belongs to: a chain leaking across subscriptions would surface as a wrong first number (or a
    subject outside the episode's root).
    """
    app = create_app(nats_url=nats_server)
    manager: EpisodeManager = app.state.manager
    episodes = {"left": 8, "right": 6}
    try:
        async with api_client(app) as http:
            for episode_id, start in episodes.items():
                created = await http.post(
                    "/applications/collatz/episodes",
                    json={"episode_id": episode_id, "config": {"starts": [start]}},
                )
                assert created.status_code == 201, created.text
            for episode_id in episodes:
                await poll_until(
                    http,
                    f"/applications/collatz/episodes/{episode_id}",
                    lambda body: body["state"] == "complete",
                )
            feeds = {
                episode_id: (
                    await http.get(f"/applications/collatz/episodes/{episode_id}/messages")
                ).json()["messages"]
                for episode_id in episodes
            }
    finally:
        await manager.shutdown()

    for episode_id, start in episodes.items():
        feed = feeds[episode_id]
        assert feed, f"episode {episode_id} served an empty feed"
        root = f"episode.collatz.{episode_id}"
        assert all(record["subject"].startswith(f"{root}.") for record in feed)
        assert all(record["payload"]["numbers"][0] == start for record in feed)


async def test_delete_stops_a_running_episode_and_its_worker(nats_server: str) -> None:
    """DELETE ends a provisioned episode: state goes to ``stopped`` and the worker is reaped.

    The delete lands immediately after the POST — well inside the worker's own startup — so the
    episode is genuinely still running when stopped.
    """
    app = create_app(nats_url=nats_server)
    manager: EpisodeManager = app.state.manager
    try:
        async with api_client(app) as http:
            created = await http.post(
                "/applications/collatz/episodes",
                json={"episode_id": "doomed", "config": {"starts": [27]}},
            )
            assert created.status_code == 201, created.text

            deleted = await http.delete("/applications/collatz/episodes/doomed")
            assert deleted.status_code == 204

            status = (await http.get("/applications/collatz/episodes/doomed")).json()
            assert status["state"] == "stopped"
    finally:
        await manager.shutdown()

    process = manager.get("collatz", "doomed").process
    assert process is not None
    assert process.poll() is not None
