"""The exported JSON Schema is the single source of truth for the viewers.

These tests pin the framework half of the contract: which model is exported,
that an opaque payload stays opaque, and that the committed artifact never
drifts from the model.
"""

from __future__ import annotations

from pathlib import Path

from freeagent.envelope import Envelope
from freeagent.schema import (
    DEFAULT_OUTPUT_DIR,
    FRAMEWORK_MODELS,
    model_schema,
    write_schemas,
)


def test_envelope_is_the_exported_framework_model() -> None:
    expected = {"envelope": Envelope}
    assert expected == FRAMEWORK_MODELS


def test_opaque_payload_exports_as_an_empty_schema() -> None:
    # An empty schema is what json-schema-to-typescript renders as `unknown` --
    # the honest type for a payload that may be a string, an object, etc. A
    # default-only node would instead render as a junk object type.
    schema = model_schema(Envelope)
    assert schema["properties"]["payload"] == {}


def test_constrained_default_is_preserved() -> None:
    # Only opaque (keyword-less) nodes collapse; a constrained field keeps its
    # default -- e.g. a discriminator a viewer narrows on.
    from typing import Literal

    from pydantic import BaseModel

    class Tagged(BaseModel):
        kind: Literal["x"] = "x"

    schema = model_schema(Tagged)
    assert schema["properties"]["kind"]["default"] == "x"


def test_committed_schema_matches_the_model(tmp_path: Path) -> None:
    # Guard against forgetting to regenerate: the checked-in artifact must equal
    # a fresh export. Regenerate with `pnpm run schemas`.
    for fresh in write_schemas(FRAMEWORK_MODELS, tmp_path):
        committed = DEFAULT_OUTPUT_DIR / fresh.name
        assert committed.read_text() == fresh.read_text(), (
            f"{committed} is stale; regenerate with `pnpm run schemas`"
        )
