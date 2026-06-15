"""The recorder process: ``python <this-file> --nats-url ... --app ... ...``.

The orchestrator spawns this file directly (a programmatic
``create_subprocess_exec`` of the interpreter on this path -- no shell, no
console script; see :func:`freeagent.cli.orchestrate._spawn_recorder`). It
parses the episode coordinates, drains the stream with :func:`record_episode`,
and writes the Parquet file.

Exits 0 on success (file written), 1 with a message on stderr on failure (e.g.
the episode's stream not found after a reasonable wait), 130 on interruption.
"""

from __future__ import annotations

import argparse
import asyncio
import sys
from pathlib import Path

from freeagent.logging import configure_logging
from freeagent.recorder.recorder import DEFAULT_IDLE_TIMEOUT, RecorderError, record_episode


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="freeagent-recorder",
        description="Drain one FreeAgent episode's JetStream stream to a Parquet file.",
    )
    parser.add_argument("--nats-url", required=True, help="NATS server URL")
    parser.add_argument("--app", required=True, help="application name (subject prefix)")
    parser.add_argument("--episode-id", required=True, help="episode id to record")
    parser.add_argument("--output", required=True, type=Path, help="Parquet output path")
    parser.add_argument(
        "--idle-timeout",
        type=float,
        default=DEFAULT_IDLE_TIMEOUT,
        help="seconds of post-shutdown silence before the recording is considered complete",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    """Parse the recorder's arguments and drain the episode to Parquet."""
    configure_logging()  # debug logging per FREEAGENT_LOG_LEVEL; infrastructure, not core
    args = _parse_args(sys.argv[1:] if argv is None else argv)
    try:
        count = asyncio.run(
            record_episode(
                nats_url=args.nats_url,
                app=args.app,
                episode_id=args.episode_id,
                output=args.output,
                idle_timeout=args.idle_timeout,
            )
        )
    except RecorderError as error:
        print(f"freeagent-recorder: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    except KeyboardInterrupt:
        print("freeagent-recorder: interrupted", file=sys.stderr)
        raise SystemExit(130) from None
    print(f"freeagent-recorder: wrote {count} rows to {args.output}")


if __name__ == "__main__":
    main()
