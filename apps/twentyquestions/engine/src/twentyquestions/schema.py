"""Export JSON Schema for Twenty Questions' wire payload models.

The framework envelope is single-sourced by :mod:`freeagent.schema`; this is
the app-level companion for the payloads a viewer needs to render -- currently
the Host's :class:`~twentyquestions.payloads.GameOver` signal. It reuses the
framework's export helpers so both halves of the contract are generated the
same way (ADR-0001), and writes alongside the engine and viewer siblings so
the contract travels with the app on a future repo split.
"""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import TYPE_CHECKING

from freeagent.schema import write_schemas

from .payloads import GameOver

if TYPE_CHECKING:
    from pydantic import BaseModel

#: ``apps/twentyquestions/schemas`` -- a sibling of ``engine`` and ``viewer``.
DEFAULT_OUTPUT_DIR = Path(__file__).resolve().parents[3] / "schemas"

#: App payload models, single-sourced here. Key is the artifact stem.
PAYLOAD_MODELS: dict[str, type[BaseModel]] = {"game_over": GameOver}


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        description="Export Twenty Questions JSON Schema from Pydantic."
    )
    parser.add_argument(
        "output_dir",
        nargs="?",
        type=Path,
        default=DEFAULT_OUTPUT_DIR,
        help=f"directory for the schema artifacts (default: {DEFAULT_OUTPUT_DIR})",
    )
    args = parser.parse_args(argv)
    for path in write_schemas(PAYLOAD_MODELS, args.output_dir):
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
