"""Tests for :mod:`freeagent.sdk.schema` and the ``freeagent schema`` CLI.

These exercise the schema-generation half of the engine→viewer pipeline against the real, workspace-
installed ``collatz`` application: ``application_schema`` builds the JSON Schema document, and
``main`` is the console-script entry point that prints it. The ``collatz`` app contributes a single
message type, :class:`~freeagent.app.collatz.message.Chain`, whose ``message_type`` tag must appear
as a ``const`` so the generated TypeScript is a discriminated union.
"""

from __future__ import annotations

import json

import pytest
from freeagent.sdk.schema import (
    NoApplicationMessages,
    _application_package,
    _owned_message_types,
    application_schema,
    main,
)


def test_application_schema_emits_chain_message_type_as_a_const() -> None:
    # The headline acceptance criterion: `freeagent schema collatz` must produce a schema in which
    # Chain.message_type is `const: "Chain"`, the discriminator TypeScript narrows a union on.
    schema = application_schema("collatz")

    assert schema["$defs"]["Chain"]["properties"]["message_type"]["const"] == "Chain"


def test_application_schema_top_level_is_a_oneof_union_over_the_apps_types() -> None:
    schema = application_schema("collatz")

    assert schema["oneOf"] == [{"$ref": "#/$defs/Chain"}]


def test_application_schema_includes_only_the_apps_own_message_types() -> None:
    # The shared registry holds SDK control-plane types (StartEntity, Ack, ...) too; the document
    # must contain the collatz app's own types only, never the SDK's.
    schema = application_schema("collatz")

    assert set(schema["$defs"]) == {"Chain"}


def test_application_schema_carries_the_apps_own_fields() -> None:
    schema = application_schema("collatz")

    assert schema["$defs"]["Chain"]["properties"]["numbers"]["type"] == "array"


def test_application_schema_marks_message_type_required() -> None:
    # An optional discriminant makes the generated union unsound: a tag-omitted payload would be
    # assignable to multiple members and could not be narrowed. The tag must be required even though
    # pydantic leaves it optional (it has a default).
    schema = application_schema("collatz")

    assert "message_type" in schema["$defs"]["Chain"]["required"]


def test_application_schema_includes_the_reserved_protocol_envelope_field() -> None:
    # `protocol` is a reserved envelope slot; it must appear in the generated schema.
    schema = application_schema("collatz")

    assert "protocol" in schema["$defs"]["Chain"]["properties"]


def test_application_schema_declares_the_json_schema_dialect() -> None:
    schema = application_schema("collatz")

    assert schema["$schema"] == "https://json-schema.org/draft/2020-12/schema"


def test_application_schema_is_json_serializable() -> None:
    # The document is printed as JSON and checked in; it must serialize without custom encoders.
    schema = application_schema("collatz")

    assert json.loads(json.dumps(schema)) == schema


def test_application_schema_raises_for_an_unknown_application() -> None:
    with pytest.raises(LookupError, match="nonesuch"):
        application_schema("nonesuch")


def test_application_package_is_the_parent_of_the_application_object_module() -> None:
    from freeagent.sdk.application import load_application

    assert _application_package(load_application("collatz")) == "freeagent.app.collatz"


def test_owned_message_types_are_sorted_by_class_name() -> None:
    # Deterministic ordering keeps the checked-in document diff-stable across runs (the CI
    # regenerate-and-diff check depends on it). The SDK package holds several control-plane types,
    # so it is a good multi-type fixture for the sort.
    types = _owned_message_types("freeagent.sdk")

    assert len(types) > 1
    assert [cls.__name__ for cls in types] == sorted(cls.__name__ for cls in types)


def test_no_application_messages_is_raised_when_a_package_defines_no_types() -> None:
    # A package with no Message subclasses of its own must fail loudly rather than emit an empty
    # oneOf that json-schema-to-typescript can't use.
    assert _owned_message_types("freeagent.sdk.entity") == []


def test_no_application_messages_is_a_valueerror() -> None:
    # It subclasses ValueError so a caller can catch it alongside other bad-input failures.
    assert issubclass(NoApplicationMessages, ValueError)


def test_main_prints_the_schema_to_stdout(capsys: pytest.CaptureFixture[str]) -> None:
    exit_code = main(["schema", "collatz"])

    assert exit_code == 0
    printed = json.loads(capsys.readouterr().out)
    assert printed["$defs"]["Chain"]["properties"]["message_type"]["const"] == "Chain"


def test_main_output_is_deterministic(capsys: pytest.CaptureFixture[str]) -> None:
    # Sorted keys + sorted types make the printed bytes stable, which the CI diff check relies on.
    main(["schema", "collatz"])
    first = capsys.readouterr().out
    main(["schema", "collatz"])
    second = capsys.readouterr().out

    assert first == second


def test_main_reports_an_unknown_application_on_stderr(
    capsys: pytest.CaptureFixture[str],
) -> None:
    exit_code = main(["schema", "nonesuch"])

    captured = capsys.readouterr()
    assert exit_code == 2
    assert "nonesuch" in captured.err
    assert captured.out == ""


def test_main_requires_a_subcommand(capsys: pytest.CaptureFixture[str]) -> None:
    # argparse exits(2) with a usage message when the required subcommand is missing.
    with pytest.raises(SystemExit):
        main([])


def test_main_lets_an_unexpected_internal_error_propagate(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # main only turns *expected* resolution / no-messages failures into exit 2. A genuine internal
    # error (here a bare TypeError, as pydantic or an import might raise) must propagate as a crash,
    # not be masked as "application can't be resolved".
    import freeagent.sdk.schema as schema_module

    def boom(_name: str) -> dict[str, object]:
        raise TypeError("internal bug, not a resolution failure")

    monkeypatch.setattr(schema_module, "application_schema", boom)

    with pytest.raises(TypeError, match="internal bug"):
        main(["schema", "collatz"])
