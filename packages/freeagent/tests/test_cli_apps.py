"""Unit tests for application self-description (``freeagent.cli.apps``).

These cover the generic-launcher contract: given a REST name, a caller obtains
everything :func:`run_episode` needs (subject prefix, environment, roster) plus
the settable config surface, and an unknown name fails cleanly. The entry-point
discovery is exercised with a fake :class:`importlib.metadata.EntryPoint` so the
test does not depend on what happens to be ``pip install``-ed.
"""

from __future__ import annotations

from dataclasses import FrozenInstanceError

import pytest
from noop_app import APP, NoopAgent, NoopEnvironment

from freeagent import (
    AppSpec,
    ConfigError,
    EpisodeConfig,
    SettableConfig,
    UnknownAppError,
    load_app,
    load_apps,
)
from freeagent.cli import apps as apps_module
from freeagent.cli.apps import (
    MANIFEST_ENTRY_POINT_GROUP,
    ManifestSpec,
    load_manifest_spec,
    load_manifest_specs,
)
from freeagent.cli.child import class_ref


class _FakeEntryPoint:
    """Just enough of ``importlib.metadata.EntryPoint`` for ``load_apps``."""

    def __init__(self, name: str, value: str, obj: object) -> None:
        self.name = name
        self.value = value
        self._obj = obj

    def load(self) -> object:
        return self._obj


def _patch_entry_points(monkeypatch: pytest.MonkeyPatch, entries: list[_FakeEntryPoint]) -> None:
    def fake_entry_points(*, group: str) -> list[_FakeEntryPoint]:
        assert group == apps_module.ENTRY_POINT_GROUP
        return entries

    monkeypatch.setattr(apps_module, "entry_points", fake_entry_points)


def test_appspec_carries_everything_run_episode_needs() -> None:
    assert APP.name == "noopapp"  # subject prefix, not the REST name
    assert APP.environment is NoopEnvironment
    assert APP.roster == {"alpha": NoopAgent}


def test_appspec_advertises_the_settable_config_surface() -> None:
    surface = APP.settable_config
    assert isinstance(surface, SettableConfig)
    # Plain, wire-safe data: only field names/types/help, no class references.
    assert [f.name for f in surface.environment] == ["episode_timeout"]
    assert "grace_period" in {f.name for f in surface.agents["alpha"]}


def test_load_apps_keys_by_rest_name(monkeypatch: pytest.MonkeyPatch) -> None:
    # REST name (dashed) differs from the subject prefix the spec carries.
    _patch_entry_points(monkeypatch, [_FakeEntryPoint("twenty-questions", "pkg.cli:APP", APP)])
    apps = load_apps()
    assert set(apps) == {"twenty-questions"}
    assert apps["twenty-questions"].name == "noopapp"


def test_load_app_returns_the_spec(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_entry_points(monkeypatch, [_FakeEntryPoint("noop", "noop_app:APP", APP)])
    assert load_app("noop") is APP


def test_load_app_unknown_name_is_a_clean_error(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_entry_points(monkeypatch, [_FakeEntryPoint("noop", "noop_app:APP", APP)])
    with pytest.raises(UnknownAppError, match=r"unknown application 'nope'.*noop") as exc:
        load_app("nope")
    assert isinstance(exc.value, ConfigError)  # part of the existing error taxonomy


def test_load_app_lists_none_when_no_apps_installed(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_entry_points(monkeypatch, [])
    with pytest.raises(UnknownAppError, match="none installed"):
        load_app("anything")


def test_load_apps_rejects_a_non_appspec_entry_point(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_entry_points(monkeypatch, [_FakeEntryPoint("broken", "noop_app:NoopAgent", NoopAgent)])
    with pytest.raises(TypeError, match="not an AppSpec"):
        load_apps()


def test_appspec_run_delegates_to_run_episode(monkeypatch: pytest.MonkeyPatch) -> None:
    """``AppSpec.run`` is the generic front door: it forwards the app's identity."""
    seen: dict[str, object] = {}

    def fake_run_episode(config: object, **kwargs: object) -> int:
        seen["config"] = config
        seen.update(kwargs)
        return 7

    monkeypatch.setattr(apps_module, "_run_episode", fake_run_episode)
    config = EpisodeConfig()
    code = APP.run(config, parquet_log=None)
    assert code == 7
    assert seen["app"] == "noopapp"
    assert seen["environment"] is NoopEnvironment
    assert seen["agents"] == {"alpha": NoopAgent}
    assert seen["config"] is config


def test_appspec_is_frozen() -> None:
    spec = AppSpec(name="x", environment=NoopEnvironment, roster={})
    with pytest.raises(FrozenInstanceError):
        spec.name = "y"  # type: ignore[misc]


def test_appspec_rejects_settable_config_for_a_non_roster_agent() -> None:
    """A settable surface naming an agent outside the roster fails at construction."""
    with pytest.raises(ValueError, match=r"settable_config names agent\(s\) \['ghost'\]"):
        AppSpec(
            name="x",
            environment=NoopEnvironment,
            roster={"alpha": NoopAgent},
            settable_config=SettableConfig(agents={"ghost": ()}),
        )


def test_appspec_allows_roster_member_with_no_settable_fields() -> None:
    """The roster/settable check is a subset: an agent may expose no settable config."""
    spec = AppSpec(
        name="x",
        environment=NoopEnvironment,
        roster={"alpha": NoopAgent, "beta": NoopAgent},
        settable_config=SettableConfig(agents={"alpha": ()}),  # beta omitted, fine
    )
    assert set(spec.roster) == {"alpha", "beta"}


# ---------------------------------------------------------------------------
# ManifestSpec: the engine-free, class-ref-string description (ADR-0005, #54)
# ---------------------------------------------------------------------------


class _FakeManifestEntryPoint:
    """Just enough of ``EntryPoint`` for ``load_manifest_specs``.

    A manifest entry point resolves to a :class:`ManifestSpec` -- the engine-free,
    class-ref-string description an app advertises on the ``freeagent.manifests``
    group. Its module carries *strings*, not engine classes, so ``load()`` imports
    no engine code (the whole point of the slim-image path).
    """

    def __init__(self, name: str, value: str, obj: object) -> None:
        self.name = name
        self.value = value
        self._obj = obj

    def load(self) -> object:
        return self._obj


def _patch_manifest_entry_points(
    monkeypatch: pytest.MonkeyPatch, entries: list[_FakeManifestEntryPoint]
) -> None:
    def fake_entry_points(*, group: str) -> list[_FakeManifestEntryPoint]:
        assert group == MANIFEST_ENTRY_POINT_GROUP
        return entries

    monkeypatch.setattr(apps_module, "entry_points", fake_entry_points)


def test_appspec_builds_its_manifest_spec_from_live_classes() -> None:
    """``AppSpec.manifest_spec`` projects the live classes onto class-ref strings."""
    manifest_spec = APP.manifest_spec()
    assert manifest_spec.name == "noopapp"
    assert manifest_spec.environment == class_ref(NoopEnvironment)
    assert manifest_spec.roster == {"alpha": class_ref(NoopAgent)}


def test_load_manifest_specs_keys_by_rest_name(monkeypatch: pytest.MonkeyPatch) -> None:
    """Discovery keys each :class:`ManifestSpec` by its dashed REST name."""
    spec = APP.manifest_spec()
    _patch_manifest_entry_points(
        monkeypatch, [_FakeManifestEntryPoint("noop", "noop_app:MANIFEST", spec)]
    )
    specs = load_manifest_specs()
    assert set(specs) == {"noop"}
    assert specs["noop"] == spec


def test_load_manifest_spec_unknown_name_is_a_clean_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    spec = APP.manifest_spec()
    _patch_manifest_entry_points(
        monkeypatch, [_FakeManifestEntryPoint("noop", "noop_app:MANIFEST", spec)]
    )
    with pytest.raises(UnknownAppError, match=r"unknown application 'nope'.*noop"):
        load_manifest_spec("nope")


def test_load_manifest_specs_rejects_a_non_manifest_entry_point(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _patch_manifest_entry_points(
        monkeypatch, [_FakeManifestEntryPoint("broken", "noop_app:NoopAgent", NoopAgent)]
    )
    with pytest.raises(TypeError, match="not a ManifestSpec"):
        load_manifest_specs()


def test_load_manifest_spec_works_when_engine_is_not_importable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The crux of the slim-image 404 fix: refs to a never-importable module resolve.

    ``ghost_engine`` is not installed anywhere; a fat worker would have it, but
    the slim service must still resolve the manifest spec (just the strings) and
    never raise an import/unknown-app error. The ManifestSpec carries the refs as
    strings, so resolving it imports nothing.
    """
    spec = ManifestSpec(
        name="ghost",
        environment="ghost_engine.env:GhostEnvironment",
        roster={"solo": "ghost_engine.agent:GhostAgent"},
    )
    _patch_manifest_entry_points(
        monkeypatch, [_FakeManifestEntryPoint("ghost-app", "ghost_manifest:MANIFEST", spec)]
    )
    resolved = load_manifest_spec("ghost-app")
    assert resolved.name == "ghost"
    assert resolved.environment == "ghost_engine.env:GhostEnvironment"
    assert resolved.roster == {"solo": "ghost_engine.agent:GhostAgent"}
