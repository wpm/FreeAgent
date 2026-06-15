"""The app half of the schema contract: the Twenty Questions payload models.

Pins which payloads are exported and that the committed artifact stays in
sync with the model, mirroring the framework guard in ``freeagent``.
"""

from __future__ import annotations

from pathlib import Path

from twentyquestions.payloads import GameOver
from twentyquestions.schema import DEFAULT_OUTPUT_DIR, PAYLOAD_MODELS, write_schemas


def test_game_over_is_the_exported_payload() -> None:
    expected = {"game_over": GameOver}
    assert expected == PAYLOAD_MODELS


def test_committed_schema_matches_the_model(tmp_path: Path) -> None:
    # Regenerate with `pnpm run schemas` if this fails.
    for fresh in write_schemas(PAYLOAD_MODELS, tmp_path):
        committed = DEFAULT_OUTPUT_DIR / fresh.name
        assert committed.read_text() == fresh.read_text(), (
            f"{committed} is stale; regenerate with `pnpm run schemas`"
        )
