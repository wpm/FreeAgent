"""Launch and supervise one episode's processes.

This is the reusable orchestration an application's ``run`` command calls. The
application supplies what it *is* in source -- its name, its environment class,
and its roster (agent name -> agent class) -- and a parsed
:class:`~freeagent.cli.config.EpisodeConfig` of per-episode tunables. The
orchestrator resolves those into a plan, validates the classes, then runs every
roster member and the environment as separate OS processes plus, optionally,
the recorder. It waits for the environment process to exit -- its exit code is
the episode outcome -- gives agents the grace period to wind down, terminates
stragglers, waits for the recorder, and prints a one-line summary.

Returns an exit code: 0 = episode ended, 2 = episode aborted, 1 = config/
launch/internal error (including operator interruption).
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import shutil
import signal
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any

from freeagent.agent import Agent
from freeagent.config import DEFAULT_GRACE_PERIOD
from freeagent.environment import Environment
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
    from collections.abc import Mapping

    from .config import EpisodeConfig

#: The child runner's source file, launched directly to avoid runpy re-executing
#: an already-imported module (see :func:`_spawn_child`).
_CHILD_SCRIPT = Path(_child_module.__file__)

RECORDER_EXECUTABLE = "freeagent-recorder"

# Extra seconds past the agents' own grace period before terminating them.
AGENT_EXIT_BUFFER = 3.0
# Seconds the recorder gets after the agents are gone (its shutdown + idle
# timeout fires on its own) before being terminated.
RECORDER_WAIT = 15.0
# Seconds between terminate() and kill() for stragglers.
TERMINATE_TIMEOUT = 5.0


def run_episode(
    config: EpisodeConfig,
    *,
    app: str,
    environment: type[Environment],
    agents: Mapping[str, type[Agent]],
) -> int:
    """Validate, then launch and supervise one episode; return its exit code.

    *app* is the application name (subject prefix); *environment* is the
    :class:`~freeagent.Environment` subclass to run; *agents* maps each roster
    name to its :class:`~freeagent.Agent` subclass. *config* holds the parsed
    per-episode tunables. Raises :class:`ConfigError` on a fatal configuration
    or launch problem.
    """
    _validate_classes(environment, agents)
    plan = make_plan(config, app=app, roster=agents)
    recorder_executable: str | None = None
    if plan.recorder_output is not None:
        recorder_executable = shutil.which(RECORDER_EXECUTABLE)
        if recorder_executable is None:
            raise ConfigError(
                f"the recorder is enabled but no {RECORDER_EXECUTABLE!r} executable "
                "was found on PATH"
            )
    return asyncio.run(_orchestrate(plan, environment, agents, recorder_executable))


def _validate_classes(environment: type[Environment], agents: Mapping[str, type[Agent]]) -> None:
    """Check the application supplied the right base classes (a programming error)."""
    if not (isinstance(environment, type) and issubclass(environment, Environment)):
        raise ConfigError(f"environment {environment!r} is not a subclass of freeagent.Environment")
    for name, cls in agents.items():
        if not (isinstance(cls, type) and issubclass(cls, Agent)):
            raise ConfigError(f"agent {name!r}: {cls!r} is not a subclass of freeagent.Agent")


async def _orchestrate(
    plan: EpisodePlan,
    environment: type[Environment],
    agents: Mapping[str, type[Agent]],
    recorder_executable: str | None,
) -> int:
    """Launch all processes, wait for the episode outcome, clean everything up."""
    loop = asyncio.get_running_loop()
    interrupted = asyncio.Event()
    handled_signals: list[signal.Signals] = []
    for sig in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(NotImplementedError, RuntimeError):
            loop.add_signal_handler(sig, interrupted.set)
            handled_signals.append(sig)
    children: list[asyncio.subprocess.Process] = []
    try:
        child_env = dict(os.environ)
        root = subject_root(plan.app, plan.episode_id)
        # Launch order doesn't matter: agents launch idle and their subscribe
        # retries until the environment has created the episode's stream.
        agent_procs: list[asyncio.subprocess.Process] = []
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
        if recorder_executable is not None and plan.recorder_output is not None:
            recorder_proc = await asyncio.create_subprocess_exec(
                recorder_executable,
                "--nats-url",
                plan.nats_url,
                "--app",
                plan.app,
                "--episode-id",
                plan.episode_id,
                "--output",
                plan.recorder_output,
                env=child_env,
            )
            children.append(recorder_proc)

        # The environment process owns the lifecycle; its exit code is the
        # episode outcome.
        env_exit = await _wait_for_environment(env_proc, interrupted)
        if env_exit is None:
            _print_summary(plan, "interrupted")
            return 1
        # Agents wind down on their own after the shutdown broadcast; give
        # them the grace period plus a buffer, then escalate.
        await _shutdown_processes(agent_procs, _agent_deadline(plan))
        if recorder_proc is not None:
            # The recorder terminates on its own after the episode ends
            # (shutdown + idle timeout); give it time, then escalate.
            await _shutdown_processes([recorder_proc], RECORDER_WAIT)
        state = _final_state(env_exit)
        _print_summary(plan, state)
        if env_exit == EXIT_ENDED:
            return 0
        if env_exit == EXIT_ABORTED:
            return 2
        return 1
    finally:
        for sig in handled_signals:
            loop.remove_signal_handler(sig)
        await _terminate_all(children)


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


async def _wait_for_environment(
    env_proc: asyncio.subprocess.Process, interrupted: asyncio.Event
) -> int | None:
    """Wait for the environment to exit; ``None`` means SIGINT/SIGTERM won."""
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


def _final_state(env_exit: int) -> str:
    """Map the environment process's exit code to the episode's final state."""
    if env_exit == EXIT_ENDED:
        return "ended"
    if env_exit == EXIT_ABORTED:
        return "aborted"
    if env_exit == EXIT_TRANSPORT:
        return "error (could not reach NATS -- is the server running?)"
    return f"error (environment exited {env_exit})"


def _print_summary(plan: EpisodePlan, state: str) -> None:
    print(f"free-agent: app={plan.app} episode_id={plan.episode_id} state={state}")
