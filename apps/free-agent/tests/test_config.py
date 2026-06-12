"""Unit tests for episode configuration parsing and validation (no NATS)."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any

import pytest
import yaml

from freeagent import NAME_PATTERN
from freeagent_runner.config import (
    DEFAULT_NATS_URL,
    ConfigError,
    EpisodeConfig,
    add_to_sys_path,
    import_class,
    load_config,
    make_plan,
    validate_classes,
)

TESTS_DIR = Path(__file__).parent


def _base_config(**overrides: Any) -> dict[str, Any]:
    config: dict[str, Any] = {
        "app": "noopapp",
        "environment": {"class": "noop_app:NoopEnvironment"},
        "agents": {
            "alpha": {"class": "noop_app:NoopAgent", "config": {"grace_period": 0.3}},
            "beta": {"class": "noop_app:NoopAgent"},
        },
    }
    config.update(overrides)
    return config


def _write(tmp_path: Path, config: dict[str, Any]) -> Path:
    path = tmp_path / "episode.yml"
    path.write_text(yaml.safe_dump(config), encoding="utf-8")
    return path


# ----------------------------------------------------------------------
# Good configurations and defaults
# ----------------------------------------------------------------------


def test_good_config_parses_with_defaults(tmp_path: Path) -> None:
    path = _write(tmp_path, _base_config())
    config = load_config(path)
    assert config.app == "noopapp"
    assert config.episode_id is None
    assert config.nats_url == DEFAULT_NATS_URL
    assert config.recorder is None
    assert config.environment.class_ref == "noop_app:NoopEnvironment"
    assert config.environment.config == {}
    assert set(config.agents) == {"alpha", "beta"}
    assert config.agents["alpha"].config == {"grace_period": 0.3}
    assert config.agents["beta"].config == {}

    plan = make_plan(config, path)
    assert NAME_PATTERN.fullmatch(plan.episode_id), "generated episode id must be subject-safe"
    assert plan.recorder_output is None
    assert plan.config_dir == tmp_path.resolve()


def test_explicit_episode_id_and_nats_url(tmp_path: Path) -> None:
    path = _write(
        tmp_path,
        _base_config(episode_id="ep-2026-06-11", nats_url="nats://elsewhere:4333"),
    )
    plan = make_plan(load_config(path), path)
    assert plan.episode_id == "ep-2026-06-11"
    assert plan.nats_url == "nats://elsewhere:4333"


def test_recorder_default_output(tmp_path: Path) -> None:
    path = _write(tmp_path, _base_config(episode_id="ep1", recorder={"enabled": True}))
    plan = make_plan(load_config(path), path)
    assert plan.recorder_output == "ep1.parquet"


def test_recorder_explicit_output(tmp_path: Path) -> None:
    recorder = {"enabled": True, "output": "out/episode.parquet"}
    path = _write(tmp_path, _base_config(recorder=recorder))
    plan = make_plan(load_config(path), path)
    assert plan.recorder_output == "out/episode.parquet"


def test_recorder_block_without_enabled_is_enabled(tmp_path: Path) -> None:
    path = _write(tmp_path, _base_config(episode_id="ep2", recorder={"output": "x.parquet"}))
    plan = make_plan(load_config(path), path)
    assert plan.recorder_output == "x.parquet"


def test_recorder_disabled(tmp_path: Path) -> None:
    path = _write(tmp_path, _base_config(recorder={"enabled": False}))
    plan = make_plan(load_config(path), path)
    assert plan.recorder_output is None


def test_environment_config_passed_verbatim(tmp_path: Path) -> None:
    environment = {
        "class": "noop_app:NoopEnvironment",
        "config": {"setup_timeout": 1.5, "episode_timeout": 12, "grace_period": 0.5},
    }
    path = _write(tmp_path, _base_config(environment=environment))
    config = load_config(path)
    assert config.environment.config == environment["config"]


# ----------------------------------------------------------------------
# Bad configurations
# ----------------------------------------------------------------------


@pytest.mark.parametrize("app", ["bad app", "no.dots", "", "wild*card"])
def test_bad_app_name_rejected(tmp_path: Path, app: str) -> None:
    path = _write(tmp_path, _base_config(app=app))
    with pytest.raises(ConfigError, match="application name"):
        load_config(path)


def test_bad_episode_id_rejected(tmp_path: Path) -> None:
    path = _write(tmp_path, _base_config(episode_id="not ok"))
    with pytest.raises(ConfigError, match="episode id"):
        load_config(path)


def test_agent_name_env_rejected(tmp_path: Path) -> None:
    agents = {"env": {"class": "noop_app:NoopAgent"}}
    path = _write(tmp_path, _base_config(agents=agents))
    with pytest.raises(ConfigError, match="reserved"):
        load_config(path)


def test_bad_agent_name_rejected(tmp_path: Path) -> None:
    agents = {"bad name!": {"class": "noop_app:NoopAgent"}}
    path = _write(tmp_path, _base_config(agents=agents))
    with pytest.raises(ConfigError, match="agent name"):
        load_config(path)


@pytest.mark.parametrize(
    "class_ref",
    ["noop_app.NoopAgent", ":NoopAgent", "noop_app:", "mod:Class.extra", "1mod:Class", "a b:C"],
)
def test_bad_class_ref_rejected(tmp_path: Path, class_ref: str) -> None:
    agents = {"alpha": {"class": class_ref}}
    path = _write(tmp_path, _base_config(agents=agents))
    with pytest.raises(ConfigError, match=re.escape("module.path:ClassName")):
        load_config(path)


def test_missing_app_rejected(tmp_path: Path) -> None:
    config = _base_config()
    del config["app"]
    with pytest.raises(ConfigError, match="app"):
        load_config(_write(tmp_path, config))


def test_missing_environment_rejected(tmp_path: Path) -> None:
    config = _base_config()
    del config["environment"]
    with pytest.raises(ConfigError, match="environment"):
        load_config(_write(tmp_path, config))


def test_missing_agents_rejected(tmp_path: Path) -> None:
    config = _base_config()
    del config["agents"]
    with pytest.raises(ConfigError, match="agents"):
        load_config(_write(tmp_path, config))


def test_empty_agents_rejected(tmp_path: Path) -> None:
    path = _write(tmp_path, _base_config(agents={}))
    with pytest.raises(ConfigError, match="at least 1"):
        load_config(path)


def test_unknown_top_level_key_rejected(tmp_path: Path) -> None:
    path = _write(tmp_path, _base_config(rooster=["alpha"]))
    with pytest.raises(ConfigError, match="rooster"):
        load_config(path)


def test_duplicate_yaml_keys_rejected(tmp_path: Path) -> None:
    path = tmp_path / "episode.yml"
    path.write_text(
        "app: noopapp\n"
        "environment:\n"
        "  class: noop_app:NoopEnvironment\n"
        "agents:\n"
        "  alpha:\n"
        "    class: noop_app:NoopAgent\n"
        "  alpha:\n"
        "    class: noop_app:NoopAgent\n",
        encoding="utf-8",
    )
    with pytest.raises(ConfigError, match="duplicate key"):
        load_config(path)


def test_top_level_must_be_mapping(tmp_path: Path) -> None:
    path = tmp_path / "episode.yml"
    path.write_text("- just\n- a\n- list\n", encoding="utf-8")
    with pytest.raises(ConfigError, match="mapping"):
        load_config(path)


def test_unreadable_file(tmp_path: Path) -> None:
    with pytest.raises(ConfigError, match="cannot read"):
        load_config(tmp_path / "does-not-exist.yml")


# ----------------------------------------------------------------------
# Class import and base-class validation
# ----------------------------------------------------------------------


def _plan(tmp_path: Path, config: dict[str, Any]) -> Any:
    path = _write(tmp_path, config)
    return make_plan(load_config(path), path)


def test_validate_classes_accepts_good_config(tmp_path: Path) -> None:
    add_to_sys_path(TESTS_DIR)
    validate_classes(_plan(tmp_path, _base_config()))


def test_environment_must_subclass_environment(tmp_path: Path) -> None:
    add_to_sys_path(TESTS_DIR)
    config = _base_config(environment={"class": "noop_app:NoopAgent"})
    with pytest.raises(ConfigError, match=r"not a subclass of freeagent\.Environment"):
        validate_classes(_plan(tmp_path, config))


def test_agent_must_subclass_agent(tmp_path: Path) -> None:
    add_to_sys_path(TESTS_DIR)
    config = _base_config(agents={"alpha": {"class": "noop_app:NotAnAgent"}})
    with pytest.raises(ConfigError, match=r"not a subclass of freeagent\.Agent"):
        validate_classes(_plan(tmp_path, config))


def test_unimportable_module(tmp_path: Path) -> None:
    config = _base_config(agents={"alpha": {"class": "no_such_module_xyz:Agent"}})
    with pytest.raises(ConfigError, match="cannot import module"):
        validate_classes(_plan(tmp_path, config))


def test_missing_class_attribute(tmp_path: Path) -> None:
    add_to_sys_path(TESTS_DIR)
    config = _base_config(agents={"alpha": {"class": "noop_app:NoSuchClass"}})
    with pytest.raises(ConfigError, match="has no attribute"):
        validate_classes(_plan(tmp_path, config))


def test_class_ref_to_non_class(tmp_path: Path) -> None:
    add_to_sys_path(TESTS_DIR)
    with pytest.raises(ConfigError, match="is not a class"):
        import_class("noop_app:NOT_A_CLASS")


def test_schema_rejects_non_mapping_agent_spec() -> None:
    raw = _base_config(agents={"alpha": "noop_app:NoopAgent"})
    with pytest.raises(ValueError, match="alpha"):
        EpisodeConfig.model_validate(raw)
