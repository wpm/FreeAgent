"""Episode tunables for the CLI: the YAML schema, validation, and the plan.

What an application *is* -- its name, its environment class, and its roster of
agent classes -- lives in the application's source code, not here. This file
parses only the per-episode tunables an operator overrides from one run to the
next::

    episode_id: ep-2026-06-11            # optional; auto-generated (uuid4 hex) when omitted
    nats_url: nats://localhost:4222      # optional, this default
    recorder:                            # optional; absent == disabled
      enabled: true                      # optional, default true when the block is present
      output: out/episode.parquet        # optional, default "<episode_id>.parquet"
    environment:                         # optional
      config: {...}                      # optional, passed verbatim to the constructor
    agents:                              # optional; keys must match the app's roster
      alice:
        config: {...}                    # optional, passed verbatim to the constructor

The agent keys are *overrides*, not the roster: an app passes its roster (the
agent names it defines in source) to :func:`make_plan`, and only those names
may appear under ``agents``. An unknown agent key is a configuration error,
caught before any process is launched.

Validation fails fast with a :class:`ConfigError`: invalid episode ids,
duplicate YAML mapping keys, unknown agent keys, and malformed blocks are all
rejected.
"""

from __future__ import annotations

import uuid
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

import yaml
from pydantic import BaseModel, ConfigDict, Field, ValidationError, field_validator

from freeagent.subjects import validate_name

if TYPE_CHECKING:
    from collections.abc import Iterable
    from pathlib import Path

DEFAULT_NATS_URL = "nats://localhost:4222"


class ConfigError(Exception):
    """A fatal problem with the episode configuration (exit code 1)."""


class RecorderSpec(BaseModel):
    """The optional ``recorder`` block; its mere presence enables the recorder."""

    model_config = ConfigDict(extra="forbid")

    enabled: bool = True
    output: str | None = None


class ComponentSpec(BaseModel):
    """A component's overrides: a verbatim constructor ``config`` dict, nothing more.

    The class is supplied by the application in source; only the config is
    tunable from the YAML.
    """

    model_config = ConfigDict(extra="forbid")

    config: dict[str, Any] = Field(default_factory=dict)


class EpisodeConfig(BaseModel):
    """The per-episode tunables, validated. Carries no class references."""

    model_config = ConfigDict(extra="forbid")

    episode_id: str | None = None
    nats_url: str = DEFAULT_NATS_URL
    recorder: RecorderSpec | None = None
    environment: ComponentSpec = Field(default_factory=ComponentSpec)
    agents: dict[str, ComponentSpec] = Field(default_factory=dict)

    @field_validator("episode_id")
    @classmethod
    def _valid_episode_id(cls, value: str | None) -> str | None:
        if value is None:
            return None
        return validate_name(value, kind="episode id")

    @field_validator("agents")
    @classmethod
    def _valid_agent_names(cls, value: dict[str, ComponentSpec]) -> dict[str, ComponentSpec]:
        for name in value:
            validate_name(name, kind="agent name")
        return value


@dataclass(frozen=True, slots=True)
class EpisodePlan:
    """A fully resolved episode: app source merged with config, ready to launch."""

    app: str
    episode_id: str
    nats_url: str
    environment_config: dict[str, Any]
    #: agent name -> its constructor config (every roster member, override applied)
    agent_configs: dict[str, dict[str, Any]]
    recorder_output: str | None  # None == recorder disabled


class _StrictLoader(yaml.SafeLoader):
    """A SafeLoader that rejects duplicate mapping keys instead of keeping the last."""

    def construct_mapping(self, node: yaml.MappingNode, deep: bool = False) -> dict[Any, Any]:
        self.flatten_mapping(node)
        seen: set[Any] = set()
        for key_node, _value_node in node.value:
            key = self.construct_object(key_node, deep=deep)
            try:
                duplicate = key in seen
                seen.add(key)
            except TypeError:  # unhashable key; let the base class complain
                continue
            if duplicate:
                raise yaml.constructor.ConstructorError(
                    "while constructing a mapping",
                    node.start_mark,
                    f"found duplicate key {key!r}",
                    key_node.start_mark,
                )
        return super().construct_mapping(node, deep)


def load_config(path: Path | None) -> EpisodeConfig:
    """Read and validate an episode configuration file.

    ``None`` (no config file given) yields an all-defaults configuration.
    Raises :class:`ConfigError` with a clear message on unreadable files,
    invalid YAML (including duplicate mapping keys), and schema violations.
    """
    if path is None:
        return EpisodeConfig()
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ConfigError(f"cannot read config file {path}: {exc}") from exc
    try:
        raw = yaml.load(text, Loader=_StrictLoader)  # a SafeLoader subclass
    except yaml.YAMLError as exc:
        raise ConfigError(f"invalid YAML in {path}: {exc}") from exc
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        raise ConfigError(f"{path}: the top level must be a mapping, not {type(raw).__name__}")
    try:
        return EpisodeConfig.model_validate(raw)
    except ValidationError as exc:
        raise ConfigError(f"invalid episode configuration in {path}:\n{exc}") from exc


def make_plan(config: EpisodeConfig, *, app: str, roster: Iterable[str]) -> EpisodePlan:
    """Resolve validated tunables against an app's roster into a launchable plan.

    *app* is the application name (the subject prefix) and *roster* is the set
    of agent names the application defines in source. Every roster member gets
    a config dict -- empty unless the YAML overrode it. An ``agents`` key that
    is not in the roster is a :class:`ConfigError`. The episode id is generated
    (uuid4 hex) when omitted, and the recorder's default output path is
    ``<episode_id>.parquet``.
    """
    roster_names = list(roster)
    roster_set = set(roster_names)
    unknown = sorted(set(config.agents) - roster_set)
    if unknown:
        known = ", ".join(sorted(roster_set)) or "(none)"
        raise ConfigError(
            f"config overrides unknown agent(s) {unknown}; this app's roster is: {known}"
        )
    episode_id = config.episode_id if config.episode_id is not None else uuid.uuid4().hex
    recorder_output: str | None = None
    if config.recorder is not None and config.recorder.enabled:
        recorder_output = (
            config.recorder.output
            if config.recorder.output is not None
            else f"{episode_id}.parquet"
        )
    agent_configs = {
        name: dict(config.agents[name].config) if name in config.agents else {}
        for name in roster_names
    }
    return EpisodePlan(
        app=app,
        episode_id=episode_id,
        nats_url=config.nats_url,
        environment_config=dict(config.environment.config),
        agent_configs=agent_configs,
        recorder_output=recorder_output,
    )
