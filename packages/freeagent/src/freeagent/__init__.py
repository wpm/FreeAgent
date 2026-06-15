"""FreeAgent: a framework for real-time multi-agent LLM interaction, no turn-taking.

An agent may speak or remain silent at any moment; everything happens in
real time. The library is substrate, not policy: it provides environments,
episodes, agents, the message envelope, and LLM plumbing -- applications
decide everything else. See DESIGN.md for the authoritative design.
"""

from __future__ import annotations

from .agent import Agent
from .cli import (
    NATS_URL_ENV_VAR,
    ConfigError,
    EpisodeConfig,
    EpisodePlan,
    ParquetLogOption,
    build_root_app,
    default_nats_url,
    load_config,
    make_plan,
    run_episode,
)
from .config import DEFAULT_EPISODE_TIMEOUT, DEFAULT_GRACE_PERIOD, DEFAULT_SETUP_TIMEOUT
from .envelope import Envelope
from .environment import Environment, EpisodeState
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
from .recorder import (
    PARQUET_SCHEMA,
    MessageRecord,
    RecorderError,
    make_record,
    record_episode,
    write_parquet,
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
    Subscription,
    Transport,
    TransportError,
)

__all__ = [
    "DEFAULT_EPISODE_TIMEOUT",
    "DEFAULT_GRACE_PERIOD",
    "DEFAULT_LOG_LEVEL",
    "DEFAULT_SETUP_TIMEOUT",
    "ENV_NAME",
    "KEY_MODELS",
    "LLM",
    "LOG_LEVEL_ENV_VAR",
    "MODEL_ENV_VAR",
    "NAME_PATTERN",
    "NATS_URL_ENV_VAR",
    "PARQUET_SCHEMA",
    "Agent",
    "ConfigError",
    "Decision",
    "Envelope",
    "Environment",
    "EpisodeConfig",
    "EpisodePlan",
    "EpisodeState",
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
    "Subscription",
    "Transport",
    "TransportError",
    "build_root_app",
    "configure_logging",
    "create_llm",
    "default_nats_url",
    "load_config",
    "log_level",
    "make_plan",
    "make_record",
    "record_episode",
    "resolve_model",
    "run_episode",
    "stream_name",
    "subject_root",
    "validate_name",
    "write_parquet",
]
