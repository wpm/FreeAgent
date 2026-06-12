"""Episode configuration for the runner: YAML schema, validation, class resolution.

The schema (names are assigned top-down from this file; agents never choose
their own)::

    app: twentyquestions                 # required, [A-Za-z0-9_-]+
    episode_id: ep-2026-06-11            # optional; auto-generated (uuid4 hex) when omitted
    nats_url: nats://localhost:4222      # optional, this default
    recorder:                            # optional; absent == disabled
      enabled: true                      # optional, default true when the block is present
      output: out/episode.parquet        # optional, default "<episode_id>.parquet"
    environment:
      class: module.path:ClassName       # required; must be an Environment subclass
      config: {...}                      # optional, passed verbatim to the constructor
    agents:                              # required, at least one; keys are the roster
      alice:
        class: module.path:ClassName     # required; must be an Agent subclass
        config: {...}                    # optional, passed verbatim to the constructor

Validation fails fast with a :class:`ConfigError`: invalid names, the
reserved agent name ``env``, malformed ``module:ClassName`` references,
duplicate YAML mapping keys, unimportable classes, and classes that are not
the required subclass are all rejected before any process is launched.
"""

from __future__ import annotations

import importlib
import re
import sys
import uuid
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

import yaml
from pydantic import BaseModel, ConfigDict, Field, ValidationError, field_validator

from freeagent import ENV_NAME, Agent, Environment, validate_name

if TYPE_CHECKING:
    from pathlib import Path

DEFAULT_NATS_URL = "nats://localhost:4222"

_CLASS_REF_PATTERN = re.compile(r"^[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*:[A-Za-z_]\w*$")


class ConfigError(Exception):
    """A fatal problem with the episode configuration (exit code 1)."""


def _check_class_ref(ref: str) -> str:
    if not _CLASS_REF_PATTERN.fullmatch(ref):
        raise ValueError(f"invalid class reference {ref!r}: expected 'module.path:ClassName'")
    return ref


class RecorderSpec(BaseModel):
    """The optional ``recorder`` block; its mere presence enables the recorder."""

    model_config = ConfigDict(extra="forbid")

    enabled: bool = True
    output: str | None = None


class EnvironmentSpec(BaseModel):
    """The ``environment`` block: class reference plus verbatim constructor config."""

    model_config = ConfigDict(extra="forbid")

    class_ref: str = Field(alias="class")
    config: dict[str, Any] = Field(default_factory=dict)

    @field_validator("class_ref")
    @classmethod
    def _valid_class_ref(cls, value: str) -> str:
        return _check_class_ref(value)


class AgentSpec(BaseModel):
    """One ``agents`` entry: class reference plus verbatim constructor config."""

    model_config = ConfigDict(extra="forbid")

    class_ref: str = Field(alias="class")
    config: dict[str, Any] = Field(default_factory=dict)

    @field_validator("class_ref")
    @classmethod
    def _valid_class_ref(cls, value: str) -> str:
        return _check_class_ref(value)


class EpisodeConfig(BaseModel):
    """The whole episode configuration file, validated."""

    model_config = ConfigDict(extra="forbid")

    app: str
    episode_id: str | None = None
    nats_url: str = DEFAULT_NATS_URL
    recorder: RecorderSpec | None = None
    environment: EnvironmentSpec
    agents: dict[str, AgentSpec] = Field(min_length=1)

    @field_validator("app")
    @classmethod
    def _valid_app(cls, value: str) -> str:
        return validate_name(value, kind="application name")

    @field_validator("episode_id")
    @classmethod
    def _valid_episode_id(cls, value: str | None) -> str | None:
        if value is None:
            return None
        return validate_name(value, kind="episode id")

    @field_validator("agents")
    @classmethod
    def _valid_agent_names(cls, value: dict[str, AgentSpec]) -> dict[str, AgentSpec]:
        for name in value:
            validate_name(name, kind="agent name")
            if name == ENV_NAME:
                raise ValueError(f"agent name {ENV_NAME!r} is reserved for the environment")
        return value


@dataclass(frozen=True, slots=True)
class EpisodePlan:
    """A fully resolved episode: defaults applied, ready to launch."""

    app: str
    episode_id: str
    nats_url: str
    environment: EnvironmentSpec
    agents: dict[str, AgentSpec]
    recorder_output: str | None  # None == recorder disabled
    config_dir: Path


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


def load_config(path: Path) -> EpisodeConfig:
    """Read and validate an episode configuration file.

    Raises :class:`ConfigError` with a clear message on unreadable files,
    invalid YAML (including duplicate mapping keys), and schema violations.
    """
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ConfigError(f"cannot read config file {path}: {exc}") from exc
    try:
        raw = yaml.load(text, Loader=_StrictLoader)  # a SafeLoader subclass
    except yaml.YAMLError as exc:
        raise ConfigError(f"invalid YAML in {path}: {exc}") from exc
    if not isinstance(raw, dict):
        raise ConfigError(f"{path}: the top level must be a mapping, not {type(raw).__name__}")
    try:
        return EpisodeConfig.model_validate(raw)
    except ValidationError as exc:
        raise ConfigError(f"invalid episode configuration in {path}:\n{exc}") from exc


def make_plan(config: EpisodeConfig, config_path: Path) -> EpisodePlan:
    """Resolve a validated configuration into a launchable plan.

    Generates the episode id (uuid4 hex) when omitted and applies the
    recorder's default output path ``<episode_id>.parquet``.
    """
    episode_id = config.episode_id if config.episode_id is not None else uuid.uuid4().hex
    recorder_output: str | None = None
    if config.recorder is not None and config.recorder.enabled:
        recorder_output = (
            config.recorder.output
            if config.recorder.output is not None
            else f"{episode_id}.parquet"
        )
    return EpisodePlan(
        app=config.app,
        episode_id=episode_id,
        nats_url=config.nats_url,
        environment=config.environment,
        agents=dict(config.agents),
        recorder_output=recorder_output,
        config_dir=config_path.resolve().parent,
    )


def add_to_sys_path(directory: Path) -> None:
    """Prepend *directory* to ``sys.path`` so app modules next to the yml import.

    The runner does the same for children via ``PYTHONPATH``.
    """
    entry = str(directory)
    if entry not in sys.path:
        sys.path.insert(0, entry)


def import_class(ref: str) -> type:
    """Import a ``module.path:ClassName`` reference; :class:`ConfigError` on failure."""
    module_name, _, class_name = ref.partition(":")
    try:
        module = importlib.import_module(module_name)
    except ImportError as exc:
        raise ConfigError(f"cannot import module {module_name!r} for {ref!r}: {exc}") from exc
    try:
        obj = getattr(module, class_name)
    except AttributeError:
        raise ConfigError(f"module {module_name!r} has no attribute {class_name!r}") from None
    if not isinstance(obj, type):
        raise ConfigError(f"{ref!r} is not a class (got {type(obj).__name__})")
    return obj


def validate_classes(plan: EpisodePlan) -> None:
    """Import every configured class in the parent and check its base class.

    The environment class must subclass :class:`freeagent.Environment`; every
    agent class must subclass :class:`freeagent.Agent`. Called before any
    child process is launched so misconfigurations fail fast.
    """
    env_class = import_class(plan.environment.class_ref)
    if not issubclass(env_class, Environment):
        raise ConfigError(
            f"environment class {plan.environment.class_ref!r} is not a subclass of "
            "freeagent.Environment"
        )
    for name, spec in plan.agents.items():
        agent_class = import_class(spec.class_ref)
        if not issubclass(agent_class, Agent):
            raise ConfigError(
                f"agent {name!r}: class {spec.class_ref!r} is not a subclass of freeagent.Agent"
            )
