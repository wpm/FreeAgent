"""Launch and supervise one episode's processes.

This is the reusable orchestration an application's ``run`` command calls. The
application supplies what it *is* in source -- its name, its environment class,
and its roster (agent name -> agent class) -- and a parsed
:class:`~freeagent.cli.config.EpisodeConfig` of per-episode tunables. The
orchestrator resolves those into a plan, validates the classes, then runs every
roster member and the environment as separate OS processes plus, optionally,
the recorder.

Two entry points, one built on the other:

* :func:`start_episode` is the primitive. It launches the child processes and
  returns an :class:`EpisodeHandle` *without blocking* -- supervision (waiting
  for the environment, winding agents down, cleaning up stragglers) runs as a
  background task the caller drives through the handle. A daemon can hold many
  handles at once in a single event loop, poll each one's state, await its
  completion, and request an abort -- with no process-wide or global state and
  no signal handlers of its own.
* :func:`run_episode` is the synchronous CLI behavior built on that primitive:
  it starts one episode, installs SIGINT/SIGTERM handlers, awaits completion,
  prints a one-line summary, and returns an exit code (0 = ended, 2 = aborted,
  1 = config/launch/internal error or operator interruption).

Signal handling is opt-in: only :func:`run_episode` installs it; an in-process
daemon caller of :func:`start_episode` gets none.
"""

from __future__ import annotations

import asyncio
import contextlib
import enum
import json
import os
import signal
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any

from freeagent.agent import Agent
from freeagent.config import DEFAULT_GRACE_PERIOD
from freeagent.environment import Environment
from freeagent.recorder import __main__ as _recorder_module
from freeagent.subjects import subject_root

from . import child as _child_module
from .child import (
    EXIT_ABORTED,
    EXIT_ENDED,
    EXIT_TRANSPORT,
    agent_spec,
    class_ref,
    environment_spec,
)
from .config import ConfigError, EpisodePlan, make_plan

if TYPE_CHECKING:
    from collections.abc import Iterator, Mapping

    from .config import EpisodeConfig

#: The child runner's source file, launched directly to avoid runpy re-executing
#: an already-imported module (see :func:`_spawn_child`).
_CHILD_SCRIPT = Path(_child_module.__file__)

#: The recorder process's source file, spawned directly (see :func:`_spawn_recorder`).
_RECORDER_SCRIPT = Path(_recorder_module.__file__)

# Extra seconds past the agents' own grace period before terminating them.
AGENT_EXIT_BUFFER = 3.0
# Seconds the recorder gets after the agents are gone (its shutdown + idle
# timeout fires on its own) before being terminated.
RECORDER_WAIT = 15.0
# Seconds between terminate() and kill() for stragglers.
TERMINATE_TIMEOUT = 5.0


class EpisodeStatus(enum.StrEnum):
    """An episode handle's lifecycle, as seen by whoever holds the handle.

    ``RUNNING`` until supervision resolves; then one terminal value. ``ENDED``,
    ``ABORTED``, and ``ERROR`` come from the environment process's exit code;
    ``INTERRUPTED`` is an operator abort (a requested abort or a CLI signal)
    that stopped the wait before the environment exited.
    """

    RUNNING = "running"
    ENDED = "ended"
    ABORTED = "aborted"
    INTERRUPTED = "interrupted"
    ERROR = "error"


@dataclass(frozen=True, slots=True)
class EpisodeOutcome:
    """The result of supervising an episode to completion.

    *status* is the terminal :class:`EpisodeStatus`; *summary* is the
    human-readable state for the one-line summary (it carries the failure detail
    that ``status`` flattens to ``ERROR``); *exit_code* is the CLI exit code
    (0 = ended, 2 = aborted, 1 = error or interruption).
    """

    status: EpisodeStatus
    summary: str
    exit_code: int


def run_episode(
    config: EpisodeConfig,
    *,
    app: str,
    environment: type[Environment],
    agents: Mapping[str, type[Agent]],
    parquet_log: Path | None = None,
) -> int:
    """Validate, then launch and supervise one episode; return its exit code.

    *app* is the application name (subject prefix); *environment* is the
    :class:`~freeagent.Environment` subclass to run; *agents* maps each roster
    name to its :class:`~freeagent.Agent` subclass. *config* holds the parsed
    per-episode tunables. When *parquet_log* is given, the recorder is spawned
    to drain this episode to that path; when ``None``, no recording happens.
    Raises :class:`ConfigError` on a fatal configuration or launch problem.

    This is the synchronous CLI behavior: it installs SIGINT/SIGTERM handlers,
    blocks until the episode finishes, prints a one-line summary, and returns
    0 (ended) / 2 (aborted) / 1 (error or operator interruption).
    """
    _validate_classes(environment, agents)
    plan = make_plan(config, app=app, roster=agents)
    return asyncio.run(_run_episode(plan, environment, agents, parquet_log))


async def _run_episode(
    plan: EpisodePlan,
    environment: type[Environment],
    agents: Mapping[str, type[Agent]],
    parquet_log: Path | None,
) -> int:
    """The CLI's supervised run: start the episode, handle signals, summarize."""
    handle = await start_episode(plan, environment, agents, parquet_log)
    with _install_signal_handlers(handle):
        outcome = await handle.wait()
    _print_summary(plan, outcome.summary)
    return outcome.exit_code


def _validate_classes(environment: type[Environment], agents: Mapping[str, type[Agent]]) -> None:
    """Check the application supplied the right base classes (a programming error)."""
    if not (isinstance(environment, type) and issubclass(environment, Environment)):
        raise ConfigError(f"environment {environment!r} is not a subclass of freeagent.Environment")
    for name, cls in agents.items():
        if not (isinstance(cls, type) and issubclass(cls, Agent)):
            raise ConfigError(f"agent {name!r}: {cls!r} is not a subclass of freeagent.Agent")


async def start_episode(
    plan: EpisodePlan,
    environment: type[Environment],
    agents: Mapping[str, type[Agent]],
    parquet_log: Path | None = None,
) -> EpisodeHandle:
    """Launch one episode's processes and return a handle, without blocking.

    Spawns every roster agent and the environment (plus the recorder when
    *parquet_log* is given) as OS processes, then starts a background task that
    supervises them to completion. The returned :class:`EpisodeHandle` lets the
    caller query state, await completion, and request an abort. No signal
    handlers are installed and no global state is touched, so many handles can
    run concurrently in one event loop.

    *plan* is an already-resolved :class:`~freeagent.cli.config.EpisodePlan`
    (see :func:`~freeagent.cli.config.make_plan`); *environment* and *agents*
    are the validated classes (see :func:`run_episode` for the validating
    front door).
    """
    child_env = dict(os.environ)
    root = subject_root(plan.app, plan.episode_id)
    agent_procs: list[asyncio.subprocess.Process] = []
    children: list[asyncio.subprocess.Process] = []
    try:
        # Launch order doesn't matter: agents launch idle and their subscribe
        # retries until the environment has created the episode's stream.
        for name, cls in agents.items():
            proc = await _spawn_child(
                agent_spec(
                    class_ref=class_ref(cls),
                    subject_root=root,
                    agent_id=name,
                    config=plan.agent_configs[name],
                    nats_url=plan.nats_url,
                ),
                child_env,
            )
            agent_procs.append(proc)
            children.append(proc)
        env_proc = await _spawn_child(
            environment_spec(
                class_ref=class_ref(environment),
                app=plan.app,
                roster=list(agents),
                episode_id=plan.episode_id,
                config=plan.environment_config,
                nats_url=plan.nats_url,
            ),
            child_env,
        )
        children.append(env_proc)
        recorder_proc: asyncio.subprocess.Process | None = None
        if parquet_log is not None:
            recorder_proc = await _spawn_recorder(plan, parquet_log, child_env)
            children.append(recorder_proc)
    except BaseException:
        # A partial launch must not leak processes: tear down what we started.
        await _terminate_all(children)
        raise
    return EpisodeHandle(
        plan=plan,
        agent_procs=agent_procs,
        env_proc=env_proc,
        recorder_proc=recorder_proc,
        children=children,
    )


class EpisodeHandle:
    """A running episode a caller drives without blocking the event loop.

    Returned by :func:`start_episode` once the child processes are up. The
    handle exposes the episode's identity (:attr:`app`, :attr:`episode_id`), its
    current :attr:`state`, and the live :attr:`processes`; it lets the caller
    :meth:`wait` for completion, read the final :attr:`outcome`, and
    :meth:`request_abort`. Supervision runs as a background task started in the
    constructor, so the episode makes progress -- agents wind down, stragglers
    are cleaned up -- whether or not anyone is awaiting it.
    """

    def __init__(
        self,
        *,
        plan: EpisodePlan,
        agent_procs: list[asyncio.subprocess.Process],
        env_proc: asyncio.subprocess.Process,
        recorder_proc: asyncio.subprocess.Process | None,
        children: list[asyncio.subprocess.Process],
    ) -> None:
        self._plan = plan
        self._agent_procs = agent_procs
        self._env_proc = env_proc
        self._recorder_proc = recorder_proc
        self._children = children
        self._interrupted = asyncio.Event()
        self._status = EpisodeStatus.RUNNING
        self._outcome: EpisodeOutcome | None = None
        # Drive supervision in the background so the caller is never blocked;
        # wait() simply awaits this task.
        self._supervisor = asyncio.ensure_future(self._supervise())

    @property
    def app(self) -> str:
        """The application name (subject prefix) this episode runs under."""
        return self._plan.app

    @property
    def episode_id(self) -> str:
        """This episode's id."""
        return self._plan.episode_id

    @property
    def state(self) -> EpisodeStatus:
        """The episode's current lifecycle state -- ``RUNNING`` until it finishes."""
        return self._status

    @property
    def processes(self) -> frozenset[asyncio.subprocess.Process]:
        """The episode's child-process set (agents, environment, optional recorder)."""
        return frozenset(self._children)

    @property
    def outcome(self) -> EpisodeOutcome | None:
        """The terminal :class:`EpisodeOutcome`, or ``None`` while still running."""
        return self._outcome

    async def wait(self) -> EpisodeOutcome:
        """Await the episode's completion and return its :class:`EpisodeOutcome`.

        Awaiting from several places is fine, and a cancelled waiter does not
        cancel the supervision (or the cleanup it owns) -- only this call's wait.
        """
        return await asyncio.shield(self._supervisor)

    def request_abort(self) -> None:
        """Ask the episode to stop now; the outcome becomes ``INTERRUPTED``.

        Idempotent and non-blocking: it wakes the supervisor, which stops
        waiting on the environment and tears every child process down. This is
        what the CLI's signal handlers call, and what a daemon calls to abort.
        """
        self._interrupted.set()

    async def _supervise(self) -> EpisodeOutcome:
        """Wait for the outcome, wind agents down, and clean every child up."""
        try:
            # The environment process owns the lifecycle; its exit code is the
            # episode outcome. ``None`` means an abort won the race first.
            env_exit = await _wait_for_environment(self._env_proc, self._interrupted)
            if env_exit is not None:
                # Agents wind down on their own after the shutdown broadcast;
                # give them the grace period plus a buffer, then escalate.
                await _shutdown_processes(self._agent_procs, _agent_deadline(self._plan))
                if self._recorder_proc is not None:
                    # The recorder terminates on its own after the episode ends
                    # (shutdown + idle timeout); give it time, then escalate.
                    await _shutdown_processes([self._recorder_proc], RECORDER_WAIT)
            outcome = _outcome_for_exit(env_exit)
            self._status = outcome.status
            self._outcome = outcome
            return outcome
        finally:
            await _terminate_all(self._children)


@contextlib.contextmanager
def _install_signal_handlers(handle: EpisodeHandle) -> Iterator[None]:
    """Route SIGINT/SIGTERM to ``handle.request_abort`` for the duration.

    Opt-in operator interruption for the CLI only; restored on exit. Platforms
    without loop signal handlers (and non-main threads) are tolerated silently.
    """
    loop = asyncio.get_running_loop()
    handled: list[signal.Signals] = []
    for sig in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(NotImplementedError, RuntimeError):
            loop.add_signal_handler(sig, handle.request_abort)
            handled.append(sig)
    try:
        yield
    finally:
        for sig in handled:
            loop.remove_signal_handler(sig)


async def _spawn_child(
    spec: dict[str, Any], child_env: dict[str, str]
) -> asyncio.subprocess.Process:
    """Launch the child runner for *spec* as its own process; stdio inherited.

    The child is launched by file path rather than ``-m freeagent.cli.child``:
    ``-m`` of a submodule first imports the ``freeagent.cli`` package, whose
    ``__init__`` already imports ``child``, so runpy then re-executes an
    already-imported module and warns. Running the file directly sidesteps that
    -- the child uses only absolute ``freeagent.*`` imports, which resolve from
    the installed package regardless of how the file was started.
    """
    return await asyncio.create_subprocess_exec(
        sys.executable,
        str(_CHILD_SCRIPT),
        json.dumps(spec, default=str),
        env=child_env,
    )


async def _spawn_recorder(
    plan: EpisodePlan, parquet_log: Path, child_env: dict[str, str]
) -> asyncio.subprocess.Process:
    """Spawn the library recorder as its own process, draining this episode to *parquet_log*.

    Programmatic ``create_subprocess_exec`` of the interpreter on the recorder
    module's file -- no shell, no console script, no PATH lookup. The recorder
    is library infrastructure; this is the only way the orchestrator launches it.
    """
    return await asyncio.create_subprocess_exec(
        sys.executable,
        str(_RECORDER_SCRIPT),
        "--nats-url",
        plan.nats_url,
        "--app",
        plan.app,
        "--episode-id",
        plan.episode_id,
        "--output",
        str(parquet_log),
        env=child_env,
    )


async def _wait_for_environment(
    env_proc: asyncio.subprocess.Process, interrupted: asyncio.Event
) -> int | None:
    """Wait for the environment to exit; ``None`` means an abort won the race."""
    env_wait = asyncio.ensure_future(env_proc.wait())
    stop_wait = asyncio.ensure_future(interrupted.wait())
    await asyncio.wait({env_wait, stop_wait}, return_when=asyncio.FIRST_COMPLETED)
    if env_wait.done():
        stop_wait.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await stop_wait
        return env_wait.result()
    env_wait.cancel()
    with contextlib.suppress(asyncio.CancelledError):
        await env_wait
    return None


def _agent_deadline(plan: EpisodePlan) -> float:
    """Seconds agents get to exit on their own: their grace period plus a buffer."""
    graces = [DEFAULT_GRACE_PERIOD]
    for config in plan.agent_configs.values():
        value = config.get("grace_period")
        if isinstance(value, int | float):
            graces.append(float(value))
    return max(graces) + AGENT_EXIT_BUFFER


async def _shutdown_processes(procs: list[asyncio.subprocess.Process], deadline: float) -> None:
    """Wait *deadline* seconds for *procs* to exit, then terminate(), then kill()."""
    if await _wait_all(procs, deadline):
        return
    _signal_all(procs, "terminate")
    if await _wait_all(procs, TERMINATE_TIMEOUT):
        return
    _signal_all(procs, "kill")
    await _wait_all(procs, TERMINATE_TIMEOUT)


async def _terminate_all(procs: list[asyncio.subprocess.Process]) -> None:
    """Final cleanup: terminate (then kill) anything still running."""
    live = [proc for proc in procs if proc.returncode is None]
    if not live:
        return
    _signal_all(live, "terminate")
    if not await _wait_all(live, TERMINATE_TIMEOUT):
        _signal_all(live, "kill")
        await _wait_all(live, TERMINATE_TIMEOUT)


async def _wait_all(procs: list[asyncio.subprocess.Process], seconds: float) -> bool:
    """True when every process has exited within *seconds*."""
    pending = [proc for proc in procs if proc.returncode is None]
    if not pending:
        return True
    waiters = [asyncio.ensure_future(proc.wait()) for proc in pending]
    _done, not_done = await asyncio.wait(waiters, timeout=seconds)
    for waiter in not_done:
        waiter.cancel()
    if not_done:
        await asyncio.gather(*not_done, return_exceptions=True)
    return not not_done


def _signal_all(procs: list[asyncio.subprocess.Process], method: str) -> None:
    """Send terminate()/kill() to every still-running process, ignoring races."""
    for proc in procs:
        if proc.returncode is None:
            with contextlib.suppress(ProcessLookupError):
                getattr(proc, method)()


def _outcome_for_exit(env_exit: int | None) -> EpisodeOutcome:
    """Map the environment exit code (``None`` = aborted) to an :class:`EpisodeOutcome`."""
    if env_exit is None:
        return EpisodeOutcome(EpisodeStatus.INTERRUPTED, "interrupted", 1)
    if env_exit == EXIT_ENDED:
        return EpisodeOutcome(EpisodeStatus.ENDED, "ended", 0)
    if env_exit == EXIT_ABORTED:
        return EpisodeOutcome(EpisodeStatus.ABORTED, "aborted", 2)
    if env_exit == EXIT_TRANSPORT:
        summary = "error (could not reach NATS -- is the server running?)"
        return EpisodeOutcome(EpisodeStatus.ERROR, summary, 1)
    return EpisodeOutcome(EpisodeStatus.ERROR, f"error (environment exited {env_exit})", 1)


def _print_summary(plan: EpisodePlan, state: str) -> None:
    print(f"free-agent: app={plan.app} episode_id={plan.episode_id} state={state}")
