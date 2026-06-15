"""Unit tests for episode-tunables parsing and the roster-merged plan (no NATS)."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
import yaml

from freeagent import NAME_PATTERN
from freeagent.cli.config import (
    ConfigError,
    EpisodeConfig,
    default_nats_url,
    load_config,
    make_plan,
)

APP = "noopapp"
ROSTER = ("alpha", "beta")


def _write(tmp_path: Path, config: dict[str, Any]) -> Path:
    path = tmp_path / "episode.yml"
    path.write_text(yaml.safe_dump(config), encoding="utf-8")
    return path


# ----------------------------------------------------------------------
# Good configurations and defaults
# ----------------------------------------------------------------------


def test_empty_config_is_all_defaults() -> None:
    config = EpisodeConfig()
    assert config.episode_id is None
    assert config.nats_url == default_nats_url()
    assert config.environment.config == {}
    assert config.agents == {}


def test_none_path_yields_defaults() -> None:
    assert load_config(None) == EpisodeConfig()


def test_empty_yaml_file_yields_defaults(tmp_path: Path) -> None:
    path = tmp_path / "episode.yml"
    path.write_text("", encoding="utf-8")
    assert load_config(path) == EpisodeConfig()


def test_good_config_parses_with_defaults(tmp_path: Path) -> None:
    path = _write(
        tmp_path,
        {"agents": {"alpha": {"config": {"grace_period": 0.3}}}},
    )
    config = load_config(path)
    assert config.nats_url == default_nats_url()
    assert config.agents["alpha"].config == {"grace_period": 0.3}

    plan = make_plan(config, app=APP, roster=ROSTER)
    assert NAME_PATTERN.fullmatch(plan.episode_id), "generated episode id must be subject-safe"
    assert plan.app == APP
    # Every roster member gets a config dict; the override applied, the rest empty.
    assert plan.agent_configs == {"alpha": {"grace_period": 0.3}, "beta": {}}
    assert plan.environment_config == {}


def test_explicit_episode_id_and_nats_url(tmp_path: Path) -> None:
    path = _write(tmp_path, {"episode_id": "ep-2026-06-11", "nats_url": "nats://elsewhere:4333"})
    plan = make_plan(load_config(path), app=APP, roster=ROSTER)
    assert plan.episode_id == "ep-2026-06-11"
    assert plan.nats_url == "nats://elsewhere:4333"


def test_environment_config_passed_verbatim(tmp_path: Path) -> None:
    env_config = {"setup_timeout": 1.5, "episode_timeout": 12, "grace_period": 0.5}
    path = _write(tmp_path, {"environment": {"config": env_config}})
    plan = make_plan(load_config(path), app=APP, roster=ROSTER)
    assert plan.environment_config == env_config


# ----------------------------------------------------------------------
# Recording is a CLI decision now, not config
# ----------------------------------------------------------------------


def test_recorder_block_in_config_is_rejected(tmp_path: Path) -> None:
    # Recording moved to the --parquet-log CLI option; the config no longer
    # carries a recorder block (extra="forbid" rejects it).
    path = _write(tmp_path, {"recorder": {"enabled": True}})
    with pytest.raises(ConfigError, match="recorder"):
        load_config(path)


# ----------------------------------------------------------------------
# Bad configurations
# ----------------------------------------------------------------------


def test_bad_episode_id_rejected(tmp_path: Path) -> None:
    path = _write(tmp_path, {"episode_id": "not ok"})
    with pytest.raises(ConfigError, match="episode id"):
        load_config(path)


def test_bad_agent_name_rejected(tmp_path: Path) -> None:
    path = _write(tmp_path, {"agents": {"bad name!": {"config": {}}}})
    with pytest.raises(ConfigError, match="agent name"):
        load_config(path)


def test_unknown_agent_key_rejected(tmp_path: Path) -> None:
    path = _write(tmp_path, {"agents": {"stranger": {"config": {}}}})
    with pytest.raises(ConfigError, match="unknown agent"):
        make_plan(load_config(path), app=APP, roster=ROSTER)


def test_unknown_top_level_key_rejected(tmp_path: Path) -> None:
    path = _write(tmp_path, {"rooster": ["alpha"]})
    with pytest.raises(ConfigError, match="rooster"):
        load_config(path)


def test_class_ref_in_config_is_rejected(tmp_path: Path) -> None:
    # Class references no longer belong in the YAML -- the roster is source.
    path = _write(tmp_path, {"environment": {"class": "noop_app:NoopEnvironment"}})
    with pytest.raises(ConfigError, match="class"):
        load_config(path)


def test_duplicate_yaml_keys_rejected(tmp_path: Path) -> None:
    path = tmp_path / "episode.yml"
    path.write_text(
        "agents:\n  alpha:\n    config: {}\n  alpha:\n    config: {}\n",
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
