"""Export JSON Schema from the framework's Pydantic wire models.

Single source of truth (ADR-0001): the message envelope is defined once in
Python (:class:`freeagent.envelope.Envelope`); its JSON Schema is exported
here as a committed artifact, and the viewers' TypeScript types are generated
from that schema -- so the cross-language seam carries no hand-maintained
duplication.

This module is both the framework's export step (``python -m freeagent.schema``)
and the home of the small helpers (:func:`model_schema`, :func:`write_schemas`)
that application-level exporters reuse for their own payload models, so both
halves of the contract are generated identically.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .envelope import Envelope

if TYPE_CHECKING:
    from pydantic import BaseModel

#: Where the committed framework schema artifacts live. Co-located with the
#: package so a future repo split carries the contract with the library.
DEFAULT_OUTPUT_DIR = Path(__file__).resolve().parents[2] / "schemas"

#: The framework wire models, single-sourced here. The key is the artifact
#: stem: ``envelope`` -> ``envelope.schema.json``.
FRAMEWORK_MODELS: dict[str, type[BaseModel]] = {"envelope": Envelope}


def _clean(node: object) -> None:
    """Recursively de-noise a Pydantic-emitted schema, in place.

    Two adjustments, both so the generated TypeScript stays honest:

    - Drop auto-generated ``title`` keys: they duplicate property names and
      only add noise. The root title is set deliberately by :func:`model_schema`.
    - Collapse an *opaque* node -- one whose only keyword is ``default`` (how
      Pydantic renders an unconstrained ``Any`` field with a default, e.g. the
      envelope ``payload``) -- to a bare empty schema. ``json-schema-to-typescript``
      renders an empty schema as ``unknown`` (correct for an opaque payload that
      may be a string, object, etc.) but renders a default-only node as a junk
      object type. A constrained field keeps its ``default`` (e.g. a discriminator).
    """
    if isinstance(node, dict):
        node.pop("title", None)
        if set(node) == {"default"}:
            del node["default"]
        for value in node.values():
            _clean(value)
    elif isinstance(node, list):
        for value in node:
            _clean(value)


def model_schema(model: type[BaseModel]) -> dict[str, Any]:
    """JSON Schema for *model*, titled by its class name and de-noised."""
    schema = model.model_json_schema()
    _clean(schema)
    schema["title"] = model.__name__
    return schema


def write_schemas(models: dict[str, type[BaseModel]], out_dir: Path) -> list[Path]:
    """Write ``<name>.schema.json`` for each model into *out_dir*.

    Returns the paths written, sorted by *models* insertion order.
    """
    out_dir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for name, model in models.items():
        path = out_dir / f"{name}.schema.json"
        path.write_text(json.dumps(model_schema(model), indent=2) + "\n")
        written.append(path)
    return written


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Export framework JSON Schema from Pydantic.")
    parser.add_argument(
        "output_dir",
        nargs="?",
        type=Path,
        default=DEFAULT_OUTPUT_DIR,
        help=f"directory for the schema artifacts (default: {DEFAULT_OUTPUT_DIR})",
    )
    args = parser.parse_args(argv)
    for path in write_schemas(FRAMEWORK_MODELS, args.output_dir):
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
