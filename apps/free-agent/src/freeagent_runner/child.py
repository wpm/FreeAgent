"""Child process entry point: ``python -m freeagent_runner.child <json-spec>``.

The runner launches every roster member and the environment as its own OS
process (agents are independent processes). Each child receives one argv
argument: a JSON spec built by :func:`agent_spec` or :func:`environment_spec`
carrying the role, the ``module:ClassName`` reference, the constructor
kwargs, and the NATS URL. The child imports the class, instantiates it with
the pinned constructor shape, and ``asyncio.run``-s it.

Exit codes: an agent child exits 0 after its ``run()`` returns; the
environment child exits 0 when the final state is ``ended`` and 2 when it is
``aborted``.
"""

from __future__ import annotations

import asyncio
import json
import sys
from typing import Any

from freeagent import EpisodeState, TransportError, configure_logging

from .config import import_class

EXIT_ENDED = 0
EXIT_ABORTED = 2
# A child that cannot reach NATS exits with this code; the runner treats it as
# a launch failure (1) rather than an episode outcome.
EXIT_TRANSPORT = 3


def agent_spec(
    *,
    class_ref: str,
    subject_root: str,
    agent_id: str,
    config: dict[str, Any],
    nats_url: str,
) -> dict[str, Any]:
    """Build the JSON-serializable child spec for one agent process."""
    return {
        "role": "agent",
        "class": class_ref,
        "subject_root": subject_root,
        "agent_id": agent_id,
        "config": config,
        "nats_url": nats_url,
    }


def environment_spec(
    *,
    class_ref: str,
    app: str,
    roster: list[str],
    episode_id: str,
    config: dict[str, Any],
    nats_url: str,
) -> dict[str, Any]:
    """Build the JSON-serializable child spec for the environment process."""
    return {
        "role": "environment",
        "class": class_ref,
        "app": app,
        "roster": roster,
        "episode_id": episode_id,
        "config": config,
        "nats_url": nats_url,
    }


def exit_code_for_state(state: EpisodeState) -> int:
    """Map the environment's final state to the child's exit code (0/2)."""
    return EXIT_ENDED if state is EpisodeState.ENDED else EXIT_ABORTED


def run_spec(spec: dict[str, Any]) -> int:
    """Instantiate and run the process described by *spec*; return its exit code."""
    role = spec["role"]
    cls = import_class(spec["class"])
    if role == "agent":
        agent = cls(
            subject_root=spec["subject_root"],
            agent_id=spec["agent_id"],
            config=spec["config"],
        )
        asyncio.run(agent.run(spec["nats_url"]))
        return 0
    if role == "environment":
        environment = cls(
            app=spec["app"],
            roster=spec["roster"],
            episode_id=spec["episode_id"],
            config=spec["config"],
        )
        state = asyncio.run(environment.run(spec["nats_url"]))
        return exit_code_for_state(state)
    raise ValueError(f"unknown child role {role!r}")


def main() -> None:
    """Parse the single JSON-spec argument and run it."""
    configure_logging()  # debug logging per FREEAGENT_LOG_LEVEL; app-level, not core
    if len(sys.argv) != 2:
        print("usage: python -m freeagent_runner.child <json-spec>", file=sys.stderr)
        raise SystemExit(1)
    spec = json.loads(sys.argv[1])
    try:
        raise SystemExit(run_spec(spec))
    except TransportError as exc:
        # One clean line instead of a connection-refused traceback; the runner
        # parent maps EXIT_TRANSPORT to its launch-error exit code.
        who = f"{spec.get('role', 'child')} {spec.get('agent_id', '')}".strip()
        print(f"free-agent: {who}: {exc}", file=sys.stderr)
        raise SystemExit(EXIT_TRANSPORT) from None


if __name__ == "__main__":
    main()
