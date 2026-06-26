"""Reusable OS-process supervision: spawn children and the terminate->kill dance.

Factored out of :mod:`freeagent.cli.orchestrate` so the worker
(:mod:`freeagent.cli.work`, ADR-0005) and the episode orchestrator share one
implementation of launching a ``child.py`` process and cleaning processes up
cooperatively (wait), then firmly (terminate), then forcibly (kill).

These helpers carry no episode policy -- no grace periods, no outcomes, no
signal handling. They are the lowest layer: given a child spec they spawn the
process; given a list of processes they wind them down. The orchestrator layers
its episode lifecycle on top; the worker layers its pull loop on top.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any

from . import child as _child_module

if TYPE_CHECKING:
    from collections.abc import Sequence

#: The child runner's source file, launched directly to avoid runpy re-executing
#: an already-imported module (see :func:`spawn_child`).
CHILD_SCRIPT = Path(_child_module.__file__)

#: Seconds between terminate() and kill() for a straggler that ignores SIGTERM.
TERMINATE_TIMEOUT = 5.0


async def spawn_child(
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
        str(CHILD_SCRIPT),
        json.dumps(spec, default=str),
        env=child_env,
    )


async def shutdown_processes(procs: Sequence[asyncio.subprocess.Process], deadline: float) -> None:
    """Wait *deadline* seconds for *procs* to exit, then terminate(), then kill()."""
    procs = list(procs)
    if await wait_all(procs, deadline):
        return
    signal_all(procs, "terminate")
    if await wait_all(procs, TERMINATE_TIMEOUT):
        return
    signal_all(procs, "kill")
    await wait_all(procs, TERMINATE_TIMEOUT)


async def terminate_all(procs: Sequence[asyncio.subprocess.Process]) -> None:
    """Final cleanup: terminate (then kill) anything still running."""
    live = [proc for proc in procs if proc.returncode is None]
    if not live:
        return
    signal_all(live, "terminate")
    if not await wait_all(live, TERMINATE_TIMEOUT):
        signal_all(live, "kill")
        await wait_all(live, TERMINATE_TIMEOUT)


async def wait_all(procs: Sequence[asyncio.subprocess.Process], seconds: float) -> bool:
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


def signal_all(procs: Sequence[asyncio.subprocess.Process], method: str) -> None:
    """Send terminate()/kill() to every still-running process, ignoring races."""
    for proc in procs:
        if proc.returncode is None:
            with contextlib.suppress(ProcessLookupError):
                getattr(proc, method)()
