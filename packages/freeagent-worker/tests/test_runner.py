"""Unit tests for :mod:`freeagent.worker.runner`'s error and teardown paths.

The happy path -- a real episode running to :class:`~freeagent.sdk.message.EpisodeComplete` over a
live server -- is covered by ``test_integration.py``. These tests instead drive
:func:`~freeagent.worker.runner.run_episode` with in-memory fakes (no NATS) to exercise the paths a
live episode does *not* reach: the timeout/force-stop branch, that force-stop tears every entity
down, that a teardown error there is swallowed rather than masking the original failure, and the
observer's ignore-everything-but-EpisodeComplete decoding.

The fakes stand in for an application and its entities: :class:`FakeApplication` hands out a
:class:`FakeEnvironment` and :class:`FakeAgent` list whose ``start``/``stop`` only record calls, so
a test controls exactly when (and whether) the observer's completion event fires by patching the
observer rather than moving real messages.
"""

from __future__ import annotations

from typing import Any

import pytest
from freeagent.sdk import EpisodeSpec
from freeagent.sdk.message import EpisodeComplete, Message, StartEntity
from freeagent.worker import runner
from freeagent.worker.runner import _EpisodeObserver, run_episode

EPISODE = EpisodeSpec(episode_root="episode.test", episode_id="test", config={})


class FakeEntity:
    """A stand-in entity that records its ``start``/``stop`` calls without touching NATS.

    :ivar started: Whether :meth:`start` has been called.
    :ivar stopped: How many times :meth:`stop` has been called (to prove idempotent force-stop).
    :ivar stop_error: If set, raised by :meth:`stop` to model a teardown that fails.
    """

    def __init__(self, stop_error: Exception | None = None) -> None:
        self.started = False
        self.stopped = 0
        self.stop_error = stop_error

    async def start(self) -> None:
        self.started = True

    async def stop(self) -> None:
        self.stopped += 1
        if self.stop_error is not None:
            raise self.stop_error


class FakeEnvironment(FakeEntity):
    """A fake environment that also records ``abort`` calls.

    The runner's force path must *abort* the environment — teardown without the
    :class:`~freeagent.sdk.message.EpisodeComplete` end marker — never ``stop()`` it, which would
    publish the marker for an episode that failed.

    :ivar aborted: How many times :meth:`abort` has been called.
    :ivar abort_error: If set, raised by :meth:`abort` to model a teardown that fails.
    """

    def __init__(self, abort_error: Exception | None = None) -> None:
        super().__init__()
        self.aborted = 0
        self.abort_error = abort_error

    async def abort(self) -> None:
        self.aborted += 1
        if self.abort_error is not None:
            raise self.abort_error


class FakeAgent(FakeEntity):
    """A fake agent: a :class:`FakeEntity` distinguished by type for readability."""


class FakeApplication:
    """A minimal application handing out pre-built fake entities.

    :ivar environment: The fake environment :meth:`make_environment` returns.
    :ivar agents: The fake agents :meth:`make_agents` returns.
    """

    name = "fake"

    def __init__(self, environment: FakeEnvironment, agents: list[FakeAgent]) -> None:
        self.environment = environment
        self.agents = agents

    def make_environment(self, episode: EpisodeSpec) -> Any:
        return self.environment

    def make_agents(self, episode: EpisodeSpec) -> Any:
        return self.agents


@pytest.fixture
def never_completing_observer(monkeypatch: pytest.MonkeyPatch) -> None:
    """Patch :class:`_EpisodeObserver` so its ``start``/``stop`` are no-ops and it never completes.

    With the observer's ``completed`` event never set, ``run_episode``'s
    ``wait_for(observer.completed.wait(), timeout)`` always hits the timeout -- the entry to the
    force-stop path these tests exercise -- without any real NATS connection.
    """

    async def _noop(self: _EpisodeObserver) -> None:
        return None

    monkeypatch.setattr(_EpisodeObserver, "start", _noop)
    monkeypatch.setattr(_EpisodeObserver, "stop", _noop)


async def test_timeout_force_stops_every_entity(never_completing_observer: None) -> None:
    environment = FakeEnvironment()
    agents = [FakeAgent(), FakeAgent(), FakeAgent()]
    application = FakeApplication(environment, agents)

    with pytest.raises(TimeoutError):
        await run_episode(application, EPISODE, "nats://x:4222", timeout=0.05)

    assert all(agent.started for agent in agents)
    assert environment.started
    assert all(agent.stopped == 1 for agent in agents)  # every agent force-stopped exactly once
    assert environment.aborted == 1  # the environment is aborted...
    assert environment.stopped == 0  # ...never stop()ped: no end marker for a failed episode


async def test_a_teardown_error_does_not_mask_the_original_failure(
    never_completing_observer: None,
) -> None:
    # The first agent's stop() raises (e.g. NoRespondersError over a severed connection). The
    # original TimeoutError must still surface, and the later entities must still be stopped.
    failing_agent = FakeAgent(stop_error=RuntimeError("no responders"))
    later_agent = FakeAgent()
    environment = FakeEnvironment()
    application = FakeApplication(environment, [failing_agent, later_agent])

    with pytest.raises(TimeoutError):  # not RuntimeError -- the teardown error is swallowed
        await run_episode(application, EPISODE, "nats://x:4222", timeout=0.05)

    assert failing_agent.stopped == 1
    assert later_agent.stopped == 1  # teardown continued past the failing agent
    assert environment.aborted == 1  # and reached the environment


async def test_an_environment_teardown_error_is_swallowed(
    never_completing_observer: None,
) -> None:
    environment = FakeEnvironment(abort_error=RuntimeError("no responders"))
    application = FakeApplication(environment, [FakeAgent()])

    with pytest.raises(TimeoutError):  # the environment abort error must not replace the timeout
        await run_episode(application, EPISODE, "nats://x:4222", timeout=0.05)

    assert environment.aborted == 1


def _decode(message: Message) -> bytes:
    return message.to_bytes()


class FakeMsg:
    """A minimal NATS message stand-in carrying just the payload the observer decodes."""

    def __init__(self, data: bytes) -> None:
        self.data = data


async def test_observer_completes_only_on_episode_complete() -> None:
    observer = _EpisodeObserver("nats://x:4222", "episode.test")

    # A non-terminal message on the environment subject is ignored: completed stays unset.
    await observer.handle_incoming_message(FakeMsg(_decode(StartEntity())))  # type: ignore[arg-type]
    assert not observer.completed.is_set()

    # An undecodable payload is likewise ignored rather than raising.
    await observer.handle_incoming_message(FakeMsg(b'{"message_type": "Nonesuch"}'))  # type: ignore[arg-type]
    assert not observer.completed.is_set()

    # The terminal EpisodeComplete sets the event.
    await observer.handle_incoming_message(FakeMsg(_decode(EpisodeComplete())))  # type: ignore[arg-type]
    assert observer.completed.is_set()


async def test_stop_quietly_swallows_and_logs(caplog: pytest.LogCaptureFixture) -> None:
    entity = FakeEntity(stop_error=RuntimeError("boom"))

    with caplog.at_level("WARNING", logger=runner.__name__):
        await runner._stop_quietly(entity)  # type: ignore[arg-type]

    assert entity.stopped == 1
    assert "boom" in caplog.text
