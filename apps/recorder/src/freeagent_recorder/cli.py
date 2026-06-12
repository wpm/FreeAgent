"""Command-line entry point for the FreeAgent episode recorder.

Usage::

    freeagent-recorder --nats-url <url> --app <app> --episode-id <id> \\
        --output <path> [--idle-timeout <secs>]

Exits 0 on success (file written), nonzero with a message on stderr on
failure (e.g. the episode's stream not found after a reasonable wait).
"""

from __future__ import annotations

import argparse
import asyncio
import sys
from pathlib import Path
from typing import TYPE_CHECKING

from freeagent_recorder.recorder import DEFAULT_IDLE_TIMEOUT, RecorderError, record_episode

if TYPE_CHECKING:
    from collections.abc import Sequence


def _build_parser() -> argparse.ArgumentParser:
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
        help="seconds of post-shutdown silence before the recording is considered "
        f"complete (default {DEFAULT_IDLE_TIMEOUT})",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> None:
    """Parse arguments, run the recorder, and exit with a process status."""
    args = _build_parser().parse_args(argv)
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
