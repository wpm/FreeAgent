"""The ``free-agent`` CLI: launch one episode's processes from a YAML config.

``free-agent run <config>.yml`` validates the configuration (names, class
references, base classes -- all in the parent, before launching anything),
then runs every roster member and the environment as separate OS processes
plus, optionally, the recorder. It waits for the environment process to exit
-- its exit code is the episode outcome -- gives agents the grace period to
wind down on their own, terminates stragglers, waits for the recorder, and
prints a one-line summary.

Exit codes: 0 = episode ended, 2 = episode aborted, 1 = config/launch/
internal error (including operator interruption).
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import json
import os
import shutil
import signal
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any

from freeagent import DEFAULT_GRACE_PERIOD, configure_logging, subject_root

from .child import EXIT_ABORTED, EXIT_ENDED, EXIT_TRANSPORT, agent_spec, environment_spec
from .config import (
    ConfigError,
    EpisodePlan,
    add_to_sys_path,
    load_config,
    make_plan,
    validate_classes,
)

if TYPE_CHECKING:
    from collections.abc import Sequence

RECORDER_EXECUTABLE = "freeagent-recorder"

# Extra seconds past the agents' own grace period before terminating them.
AGENT_EXIT_BUFFER = 3.0
# Seconds the recorder gets after the agents are gone (its shutdown + idle
# timeout fires on its own) before being terminated.
RECORDER_WAIT = 15.0
# Seconds between terminate() and kill() for stragglers.
TERMINATE_TIMEOUT = 5.0


def main(argv: Sequence[str] | None = None) -> None:
    """Console entry point (sync): parse arguments, run, exit with status."""
    configure_logging()  # debug logging per FREEAGENT_LOG_LEVEL; app-level, not core
    args = _parse_args(argv)
    try:
        code = _run(args.config)
    except ConfigError as exc:
        print(f"free-agent: error: {exc}", file=sys.stderr)
        code = 1
    except KeyboardInterrupt:
        print("free-agent: interrupted", file=sys.stderr)
        code = 1
    sys.exit(code)


def _parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="free-agent",
        description="Launch a FreeAgent episode (environment, agents, optional recorder) "
        "from a YAML configuration.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    run_parser = subparsers.add_parser("run", help="run one episode from a YAML configuration")
    run_parser.add_argument("config", type=Path, help="episode configuration *.yml file")
    return parser.parse_args(argv)


def _run(config_path: Path) -> int:
    """Validate the configuration, then orchestrate the episode."""
    config = load_config(config_path)
    plan = make_plan(config, config_path)
    # App modules living next to the yml are importable in the parent (for
    # validation) and in every child (via PYTHONPATH).
    add_to_sys_path(plan.config_dir)
    validate_classes(plan)
    recorder_executable: str | None = None
    if plan.recorder_output is not None:
        recorder_executable = shutil.which(RECORDER_EXECUTABLE)
        if recorder_executable is None:
            raise ConfigError(
                f"the recorder is enabled but no {RECORDER_EXECUTABLE!r} executable "
                "was found on PATH"
            )
    return asyncio.run(_orchestrate(plan, recorder_executable))


async def _orchestrate(plan: EpisodePlan, recorder_executable: str | None) -> int:
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
        child_env = _child_environ(plan.config_dir)
        root = subject_root(plan.app, plan.episode_id)
        # Launch order doesn't matter: agents launch idle and their subscribe
        # retries until the environment has created the episode's stream.
        agent_procs: list[asyncio.subprocess.Process] = []
        for name, spec in plan.agents.items():
            proc = await _spawn_child(
                agent_spec(
                    class_ref=spec.class_ref,
                    subject_root=root,
                    agent_id=name,
                    config=spec.config,
                    nats_url=plan.nats_url,
                ),
                child_env,
            )
            agent_procs.append(proc)
            children.append(proc)
        env_proc = await _spawn_child(
            environment_spec(
                class_ref=plan.environment.class_ref,
                app=plan.app,
                roster=list(plan.agents),
                episode_id=plan.episode_id,
                config=plan.environment.config,
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


def _child_environ(config_dir: Path) -> dict[str, str]:
    """The children's environment: PYTHONPATH led by the config file's directory."""
    env = dict(os.environ)
    existing = env.get("PYTHONPATH")
    env["PYTHONPATH"] = str(config_dir) if not existing else f"{config_dir}{os.pathsep}{existing}"
    return env


async def _spawn_child(
    spec: dict[str, Any], child_env: dict[str, str]
) -> asyncio.subprocess.Process:
    """Launch ``python -m freeagent_runner.child <json-spec>``; stdio inherited."""
    return await asyncio.create_subprocess_exec(
        sys.executable,
        "-m",
        "freeagent_runner.child",
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
    for spec in plan.agents.values():
        value = spec.config.get("grace_period")
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


if __name__ == "__main__":
    main()
