"""Export JSON Schema for the episode-service contract models (ADR-0003).

The episode REST API and the per-episode feed are the **contract** between the
service and any UI. Like the framework envelope (:mod:`freeagent.schema`), the
contract is single-sourced from Pydantic and exported here as committed JSON
Schema artifacts, from which the UI's TypeScript types are generated -- so the
cross-language seam carries no hand-maintained duplication.

Run as ``python -m freeagent.service.schema`` (wired into ``pnpm run schemas``);
the artifacts land beside the framework's, so the viewer's ``json2ts`` glob picks
them up with no extra configuration.
"""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import TYPE_CHECKING

from freeagent.schema import DEFAULT_OUTPUT_DIR, write_schemas

from .models import (
    CreateEpisodeRequest,
    EpisodeView,
    ExportEpisodeRequest,
    ExportEpisodeResult,
    FeedConnectionEvent,
    FeedMessageEvent,
    FeedStatusEvent,
    ImportEpisodeRequest,
    RenameEpisodeRequest,
    TeardownResult,
)

if TYPE_CHECKING:
    from pydantic import BaseModel

#: The contract models, keyed by artifact stem (``episode_view`` ->
#: ``episode_view.schema.json``). Nested models (``ComponentConfig`` inside a
#: create request, ``EpisodeMessage`` inside a message event) ride along in each
#: file's ``$defs`` and are generated as named TypeScript interfaces.
CONTRACT_MODELS: dict[str, type[BaseModel]] = {
    "episode_view": EpisodeView,
    "create_episode_request": CreateEpisodeRequest,
    "rename_episode_request": RenameEpisodeRequest,
    "import_episode_request": ImportEpisodeRequest,
    "export_episode_request": ExportEpisodeRequest,
    "export_episode_result": ExportEpisodeResult,
    "feed_message_event": FeedMessageEvent,
    "feed_status_event": FeedStatusEvent,
    "feed_connection_event": FeedConnectionEvent,
    "teardown_result": TeardownResult,
}


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        description="Export episode-service contract JSON Schema from Pydantic."
    )
    parser.add_argument(
        "output_dir",
        nargs="?",
        type=Path,
        default=DEFAULT_OUTPUT_DIR,
        help=f"directory for the schema artifacts (default: {DEFAULT_OUTPUT_DIR})",
    )
    args = parser.parse_args(argv)
    for path in write_schemas(CONTRACT_MODELS, args.output_dir):
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
