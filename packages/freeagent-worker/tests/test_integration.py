"""End-to-end integration test: the worker runs a complete Collatz episode over a real NATS server.

This is the epic's first over-the-wire proof (issue #80). Everything above it -- the SDK's entities,
the Collatz application, the loader, the worker's runner and CLI -- is exercised together against a
real ``nats-server`` subprocess, because this is the level at which subject-routing and lifecycle-
ordering bugs live: exactly the class of bug the rewrite's testing discipline exists to catch, and
the class a fake NATS client cannot surface (a fake acks instantly, so it hides the run- loop-ack
ordering that deadlocks a multi-agent episode over a real server).

A passive *wire tap* -- a second NATS connection subscribed to the whole episode subtree -- records
every message the episode puts on the wire, so the test can assert on the control-plane traffic
(``StopAgent`` per finished agent, exactly one ``EpisodeComplete``) rather than only on the runner
returning. The episode is driven both directly through :func:`~freeagent.worker.runner.run_episode`
(to inspect the wire) and through the CLI (to prove the process exits cleanly), covering the
acceptance criteria end to end.

The test carries the :func:`pytest.mark.integration` marker, so ``pytest -m "not integration"``
skips it (and its NATS dependency) with the rest of the integration tier.
"""

from __future__ import annotations

import asyncio
import json

import nats
import pytest
from freeagent.app.collatz.message import Chain, is_complete
from freeagent.sdk import EpisodeSpec, load_application
from freeagent.sdk.message import EpisodeComplete, Message, StopAgent
from freeagent.worker.cli import app
from freeagent.worker.runner import run_episode
from nats.aio.msg import Msg
from typer.testing import CliRunner

pytestmark = pytest.mark.integration

# Three starts of visibly different chain length, so the agents finish in different orders and the
# episode genuinely exercises concurrent agents rather than a lockstep march. 8 finishes in three
# steps (8 -> 4 -> 2 -> 1); 27 is famously long (111 steps). The set satisfies the acceptance
# criterion of "at least 3 agents with different chain lengths".
STARTS = [8, 6, 27]


class WireTap:
    """A passive second NATS connection recording every message on an episode's subtree.

    Subscribes to ``{episode_root}.>`` and decodes each message it sees, so a test can count the
    control-plane traffic (``StopAgent``, ``EpisodeComplete``) the episode produced. It never
    replies or publishes -- it is invisible to the episode -- so what it records is exactly what
    crossed the wire.

    :ivar messages: Each decoded message seen as ``(subject, message)``, in arrival order;
        undecodable payloads are skipped.
    """

    def __init__(self) -> None:
        self.messages: list[tuple[str, Message]] = []

    async def start(self, servers: str, episode_root: str) -> None:
        """Connect and subscribe to the episode's whole subject subtree.

        :param servers: The NATS server URL to connect to.
        :param episode_root: The episode root whose subtree (``{episode_root}.>``) to tap.
        """
        self._client = await nats.connect(servers)
        await self._client.subscribe(f"{episode_root}.>", cb=self._record)

    async def _record(self, msg: Msg) -> None:
        message = Message.try_decode(msg.data)
        if message is not None:
            self.messages.append((msg.subject, message))

    async def stop(self) -> None:
        """Disconnect the tap's own NATS connection."""
        await self._client.close()

    def count(self, message_type: type[Message]) -> int:
        """Count recorded messages of a given concrete type.

        :param message_type: The message class to count instances of.
        :return: How many recorded messages are of exactly that type.
        """
        return sum(isinstance(message, message_type) for _, message in self.messages)

    def agents_that_completed_a_chain(self) -> set[str]:
        """Names of agents whose reply subject carried a chain that reached 1.

        A completed chain returning on ``{episode_root}.environment.replies.{agent}`` is the wire
        evidence that that agent's chain ran to the Collatz fixed point -- i.e. the agent finished.

        :return: The set of agent names seen returning a completed :class:`Chain`.
        """
        completed: set[str] = set()
        for subject, message in self.messages:
            if isinstance(message, Chain) and ".replies." in subject and is_complete(message):
                completed.add(subject.rsplit(".", 1)[-1])
        return completed


async def test_worker_runs_a_full_collatz_episode_over_a_real_server(nats_server: str) -> None:
    """A complete Collatz episode runs end to end, with the expected control-plane traffic.

    Runs three agents of different chain lengths through the real worker runner against a real
    server, tapping the wire to assert every acceptance-criteria invariant: every agent's chain
    completes (each returns a chain reaching 1), each finished agent is individually stopped with
    ``StopAgent``, and exactly one ``EpisodeComplete`` marks the end. The runner returning at all is
    itself the "environment tears down / process exits cleanly" invariant, since it blocks until the
    terminal ``EpisodeComplete`` and then disconnects everything.
    """
    episode_root = "episode.integration"
    tap = WireTap()
    await tap.start(nats_server, episode_root)

    application = load_application("collatz")
    episode = EpisodeSpec(
        episode_root=episode_root,
        episode_id="integration",
        config={"starts": STARTS, "servers": nats_server},
    )

    await asyncio.wait_for(run_episode(application, episode, nats_server, timeout=30), timeout=45)

    # Give the tap a moment to drain any trailing frames still in flight after the runner returned.
    await asyncio.sleep(0.2)
    await tap.stop()

    # Every agent's chain ran to 1 -- each agent started, was seeded, and finished.
    assert tap.agents_that_completed_a_chain() == {f"agent-{i}" for i in range(len(STARTS))}
    # Each finished agent was stopped individually with StopAgent as its chain reached 1.
    assert tap.count(StopAgent) == len(STARTS)
    # Exactly one end marker, published once at the end of the episode.
    assert tap.count(EpisodeComplete) == 1


def test_cli_runs_a_full_episode_and_exits_cleanly(nats_server: str) -> None:
    """``freeagent-worker run collatz ...`` completes an episode against a live server, exit code 0.

    This is the acceptance-criterion command driven through the real CLI (via Typer's runner)
    against the real server, proving the whole path -- argument parsing, application loading, spec
    building, episode run, teardown -- ends with a clean exit rather than only that the runner
    coroutine returns.
    """
    config = json.dumps({"starts": STARTS, "servers": nats_server})
    result = CliRunner().invoke(
        app,
        [
            "run",
            "collatz",
            "--episode-id",
            "57",
            "--nats-url",
            nats_server,
            "--config",
            config,
            "--timeout",
            "30",
        ],
    )

    assert result.exit_code == 0, result.output
    assert "complete" in result.stdout
