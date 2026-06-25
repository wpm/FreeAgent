"""FreeAgent: a framework for real-time multi-agent LLM interaction, no turn-taking.

An agent may speak or remain silent at any moment; everything happens in
real time. The library is substrate, not policy: it provides environments,
episodes, agents, the message envelope, and LLM plumbing -- applications
decide everything else. See DESIGN.md for the authoritative design.
"""

from __future__ import annotations

from .agent import Agent
from .cli import (
    AGENT_FIELDS,
    ENVIRONMENT_FIELDS,
    LLM_AGENT_FIELDS,
    NATS_URL_ENV_VAR,
    AppSpec,
    ConfigError,
    ConfigField,
    EpisodeConfig,
    EpisodeHandle,
    EpisodeOutcome,
    EpisodePlan,
    EpisodeStatus,
    ParquetLogOption,
    SettableConfig,
    UnknownAppError,
    build_root_app,
    default_nats_url,
    load_app,
    load_apps,
    load_config,
    make_plan,
    run_episode,
    start_episode,
)
from .config import DEFAULT_EPISODE_TIMEOUT, DEFAULT_GRACE_PERIOD, DEFAULT_SETUP_TIMEOUT
from .control import (
    OPERATOR_SENDER,
    abort_episode,
    operator_abort_message,
    publish_operator_abort,
)
from .envelope import Envelope
from .environment import OPERATOR_ABORT_TYPE, Environment, EpisodeState
from .llm import (
    KEY_MODELS,
    LLM,
    MODEL_ENV_VAR,
    FakeLLM,
    FakeLLMError,
    ModelResolutionError,
    create_llm,
    resolve_model,
)
from .llm_agent import Decision, LLMAgent
from .logging import (
    DEFAULT_LOG_LEVEL,
    LOG_LEVEL_ENV_VAR,
    configure_logging,
    log_level,
)
from .metadata import EpisodeMetadata
from .names import fallback_episode_name
from .recorder import (
    PARQUET_SCHEMA,
    MessageRecord,
    RecorderError,
    make_record,
    record_episode,
    write_parquet,
)
from .replayer import (
    Replayer,
    ReplayerError,
    ReplayMessage,
    load_episode,
    replay_episode,
)
from .subjects import (
    ENV_NAME,
    NAME_PATTERN,
    EpisodeSubjects,
    stream_name,
    subject_root,
    validate_name,
)
from .transport import (
    MemoryTransport,
    NatsTransport,
    SealedStreamError,
    Subscription,
    Transport,
    TransportError,
)

__all__ = [
    "AGENT_FIELDS",
    "DEFAULT_EPISODE_TIMEOUT",
    "DEFAULT_GRACE_PERIOD",
    "DEFAULT_LOG_LEVEL",
    "DEFAULT_SETUP_TIMEOUT",
    "ENVIRONMENT_FIELDS",
    "ENV_NAME",
    "KEY_MODELS",
    "LLM",
    "LLM_AGENT_FIELDS",
    "LOG_LEVEL_ENV_VAR",
    "MODEL_ENV_VAR",
    "NAME_PATTERN",
    "NATS_URL_ENV_VAR",
    "OPERATOR_ABORT_TYPE",
    "OPERATOR_SENDER",
    "PARQUET_SCHEMA",
    "Agent",
    "AppSpec",
    "ConfigError",
    "ConfigField",
    "Decision",
    "Envelope",
    "Environment",
    "EpisodeConfig",
    "EpisodeHandle",
    "EpisodeMetadata",
    "EpisodeOutcome",
    "EpisodePlan",
    "EpisodeState",
    "EpisodeStatus",
    "EpisodeSubjects",
    "FakeLLM",
    "FakeLLMError",
    "LLMAgent",
    "MemoryTransport",
    "MessageRecord",
    "ModelResolutionError",
    "NatsTransport",
    "ParquetLogOption",
    "RecorderError",
    "ReplayMessage",
    "Replayer",
    "ReplayerError",
    "SealedStreamError",
    "SettableConfig",
    "Subscription",
    "Transport",
    "TransportError",
    "UnknownAppError",
    "abort_episode",
    "build_root_app",
    "configure_logging",
    "create_llm",
    "default_nats_url",
    "fallback_episode_name",
    "load_app",
    "load_apps",
    "load_config",
    "load_episode",
    "log_level",
    "make_plan",
    "make_record",
    "operator_abort_message",
    "publish_operator_abort",
    "record_episode",
    "replay_episode",
    "resolve_model",
    "run_episode",
    "start_episode",
    "stream_name",
    "subject_root",
    "validate_name",
    "write_parquet",
]
