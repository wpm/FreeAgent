"""Generate a JSON Schema document for an application's message vocabulary.

Python is the single source of truth for the shapes a viewer needs: rather than
hand-maintaining TypeScript, an application *generates* it — pydantic ``model_json_schema()`` → one
JSON Schema document per application → ``json-schema-to-typescript`` in the viewer build. This
module is the first half of that pipeline: :func:`application_schema` builds the document, and the
``freeagent schema <application>`` CLI (:func:`main`) prints it.

Loading the application registers its :class:`~freeagent.sdk.message.Message` subclasses in the open
``Message._by_type`` registry (via ``__pydantic_init_subclass__``). That registry is shared across
everything imported into the process — SDK control-plane types plus every other loaded application —
so the document must include only the *target* application's own message types. They are selected by
defining module: a type belongs to application ``collatz`` if its module is inside the
``freeagent.app.collatz`` package the application's class lives in. That keeps one app's schema from
bleeding in another's (or the SDK's) types.

Each message type's ``message_type`` tag is emitted as a ``const`` (see
:meth:`~freeagent.sdk.message.Message.__pydantic_init_subclass__`), so the generated TypeScript is a
discriminated union a viewer can narrow with ``msg.message_type === "Chain"``.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from typing import Any

from freeagent.sdk.application import (
    AmbiguousApplication,
    Application,
    InvalidApplication,
    UnknownApplication,
    load_application,
)
from freeagent.sdk.message import Message
from pydantic.json_schema import models_json_schema

SCHEMA_DIALECT = "https://json-schema.org/draft/2020-12/schema"
"""The JSON Schema dialect declared in the generated document's ``$schema``.

Pinned so downstream tools (``json-schema-to-typescript``) and human readers agree on the meta-
schema the document conforms to, rather than leaving it implicit.
"""


def _application_package(application: Application) -> str:
    """Return the Python package an application's own message types live under.

    The application object is defined in a module inside its package (e.g. Collatz's ``application``
    object lives in ``freeagent.app.collatz.application``); its message types are siblings under the
    same package (``freeagent.app.collatz.message``). The package that contains both is the module
    of the application object's class with its final component dropped — ``freeagent.app.collatz`` —
    which is the prefix :func:`_owned_message_types` selects on.

    This assumes the conventional layout: the ``Application`` object's class and the application's
    ``Message`` subclasses live in sibling modules of one package. That holds for apps generated
    from the standard structure; an app that registers an ``Application`` instance whose *class* is
    defined outside its own package (e.g. a generic base class in a shared module) would resolve to
    the wrong package here, and :func:`application_schema` would then fail with
    :class:`NoApplicationMessages` naming the package it searched. If that layout ever becomes real,
    this is the single spot to revisit — e.g. deriving the package from the registered entry-point
    value instead of the class module.

    :param application: The loaded application object.
    :return: The dotted package name the application's message types are defined under.
    """
    return type(application).__module__.rpartition(".")[0]


def _owned_message_types(package: str) -> list[type[Message]]:
    """Select the registered :class:`~freeagent.sdk.message.Message` subclasses defined in a
    package.

    Walks the shared ``Message._by_type`` registry — which holds SDK types plus every message class
    imported into the process — and keeps only those whose defining module is ``package`` itself or
    nested under it. This registry-walk filter ensures an application's schema contains only its own
    message vocabulary, never the SDK's control-plane types or another loaded application's. The
    result is sorted by class name for a stable, deterministic document.

    :param package: The dotted package name to select types from, from :func:`_application_package`.
    :return: The application's own message types, sorted by class name.
    """
    prefix = f"{package}."
    owned = [
        cls
        for cls in Message._by_type.values()
        if cls.__module__ == package or cls.__module__.startswith(prefix)
    ]
    return sorted(owned, key=lambda cls: cls.__name__)


def application_schema(name: str) -> dict[str, Any]:
    """Build the JSON Schema document describing an application's message vocabulary.

    Loads the application by name — which imports its package and registers its message types — then
    emits one document whose ``$defs`` hold a schema per message type (each with its
    ``message_type`` tag as a ``const``) and whose top level is a ``oneOf`` union over them, so
    ``json-schema-to-typescript`` produces a discriminated union a viewer narrows on the tag.

    Each message type's ``message_type`` is forced into its ``required`` list. pydantic leaves it
    optional because it has a default, but an *optional* discriminant makes the generated union
    unsound: a payload that omits the tag would be assignable to more than one member and
    ``msg.message_type === "..."`` could not narrow it. Making the tag required matches the wire
    reality (:meth:`~freeagent.sdk.message.Message.to_bytes` always emits it) and yields a sound
    discriminated union. See :func:`_require_message_type`.

    :param name: The application's bare name, e.g. ``collatz``.
    :return: The application's message schema as a JSON-serializable ``dict``.
    :raises UnknownApplication: If no installed application registered under ``name``.
    :raises AmbiguousApplication: If more than one installed application registered under ``name``.
    :raises InvalidApplication: If ``name`` resolves to an object that isn't a valid
        :class:`~freeagent.sdk.application.Application`.
    :raises NoApplicationMessages: If the application defines no message types of its own.
    """
    application = load_application(name)
    package = _application_package(application)
    types = _owned_message_types(package)
    if not types:
        raise NoApplicationMessages(
            f'Application "{name}" defines no Message subclasses under package "{package}"; '
            f"nothing to generate. (The search package is derived from the application class's "
            f"module; see freeagent.sdk.schema._application_package for the layout it assumes.)"
        )
    # models_json_schema returns (per-model map keyed by (model, mode), shared definitions); the
    # definitions dict is what carries the `$defs` block we reference from the top-level union.
    _, definitions = models_json_schema(
        [(cls, "validation") for cls in types],
        ref_template="#/$defs/{model}",
    )
    defs = definitions["$defs"]
    for definition in defs.values():
        _require_message_type(definition)
    return {
        "$schema": SCHEMA_DIALECT,
        "title": f"{name} messages",
        "description": (
            f'Generated message schema for the "{name}" Free Agent application. Source of truth is '
            "the application's pydantic models; regenerate with `freeagent schema` rather than "
            "editing by hand."
        ),
        "oneOf": [{"$ref": f"#/$defs/{cls.__name__}"} for cls in types],
        "$defs": defs,
    }


def _require_message_type(definition: dict[str, Any]) -> None:
    """Mark a message type's ``message_type`` discriminant as required, in place.

    pydantic emits the tag as non-required because it has a default; an optional discriminant makes
    the generated TypeScript union unsound (see :func:`application_schema`). This adds
    ``message_type`` to the definition's ``required`` list if the definition declares the field and
    doesn't already require it. Defensive about the shape — it only touches definitions that
    actually have a ``message_type`` property — so it is a no-op on anything unexpected rather than
    a crash.

    :param definition: One entry from the schema's ``$defs``, mutated in place.
    """
    properties = definition.get("properties")
    if not isinstance(properties, dict) or "message_type" not in properties:
        return
    required = definition.setdefault("required", [])
    if "message_type" not in required:
        required.append("message_type")
        required.sort()


class NoApplicationMessages(ValueError):
    """Raised when an application defines no message types of its own to generate a schema from.

    An application with no :class:`~freeagent.sdk.message.Message` subclasses in its own package
    yields an empty document with an empty ``oneOf``, which ``json-schema-to-typescript`` can't turn
    into a useful union. Treated as an error so the CLI fails loudly rather than emitting a
    degenerate schema.
    """


def main(argv: Sequence[str] | None = None) -> int:
    """Entry point for the ``freeagent`` console script.

    Parses arguments and dispatches the ``schema`` subcommand, printing the generated document to
    standard output as indented JSON with a trailing newline. Kept deterministic (sorted types,
    stable key order) so the output is safe to check in and diff in CI, which regenerates the
    schema and fails on any difference.

    :param argv: The argument vector, excluding the program name; defaults to ``sys.argv[1:]``.
    :return: A process exit code: ``0`` on success, or ``2`` for an expected user-input failure —
        the application can't be resolved (:class:`~freeagent.sdk.application.UnknownApplication`,
        :class:`~freeagent.sdk.application.AmbiguousApplication`,
        :class:`~freeagent.sdk.application.InvalidApplication`) or resolves but defines no message
        types of its own (:class:`NoApplicationMessages`). Any other exception is an internal error
        and is left to propagate with its traceback rather than being reported as exit ``2``.
    """
    parser = argparse.ArgumentParser(
        prog="freeagent",
        description="Free Agent SDK command line tools.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    schema_parser = subparsers.add_parser(
        "schema",
        help="Emit the JSON Schema for an application's message vocabulary.",
    )
    schema_parser.add_argument(
        "application",
        help="The bare name of the application, e.g. `collatz`.",
    )
    args = parser.parse_args(argv)

    try:
        document = application_schema(args.application)
    except (
        UnknownApplication,
        AmbiguousApplication,
        InvalidApplication,
        NoApplicationMessages,
    ) as error:
        # Only the expected resolution / no-messages failures become a clean exit-2 message; a bare
        # LookupError or TypeError from a genuine internal bug is deliberately *not* caught, so it
        # surfaces as a traceback instead of masquerading as "application can't be resolved".
        print(f"freeagent schema: {error}", file=sys.stderr)
        return 2
    print(json.dumps(document, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
