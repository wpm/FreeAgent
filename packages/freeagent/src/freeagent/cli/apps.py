"""Self-description of a FreeAgent application, for a generic launcher.

An application *is* its source: the subject-prefix name, the environment class,
and the roster (agent name -> agent class) are defined in the app, not in a
config file (see ``apps/twentyquestions``). The per-app ``run`` command already
hands those to :func:`freeagent.run_episode`. But a generic caller -- the
control service -- must launch an episode of an *arbitrary* installed app
without importing it by name, so the app has to *advertise* the same facts
through a discoverable channel.

That channel is the existing ``freeagent.apps`` entry-point group. Instead of
pointing the entry point at a bare Typer sub-app (enough to mount CLI commands,
not enough to launch), an application registers an :class:`AppSpec`::

    # in the application's pyproject.toml
    [project.entry-points."freeagent.apps"]
    twenty-questions = "twentyquestions.cli:APP"   # APP is an AppSpec

The entry-point *name* (``twenty-questions``, dashed) is the REST name; the
spec's :attr:`~AppSpec.name` (``twentyquestions``, undashed) is the subject
prefix. The two differ, and the canonical mapping between them is defined once,
here: :func:`load_apps` keys every installed :class:`AppSpec` by its REST name,
and each spec carries its own subject prefix. The library still never imports
its applications by name -- it only reads what they registered.

An :class:`AppSpec` carries everything :func:`freeagent.run_episode` needs plus
the **settable config surface**: which ``config`` fields an operator may set on
the environment and each roster member (the Host's secret, a model override,
and so on). That surface is plain, wire-safe data -- field names, JSON-ish type
tags, and help text -- with no class references, so it can cross a REST boundary
or seed YAML the same way :class:`~freeagent.cli.config.ComponentSpec`'s
``config`` does.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from importlib.metadata import entry_points
from typing import TYPE_CHECKING

from freeagent.agent import Agent
from freeagent.environment import Environment

from .child import class_ref
from .config import ConfigError
from .orchestrate import run_episode as _run_episode

if TYPE_CHECKING:
    from collections.abc import Mapping
    from pathlib import Path

    import typer

    from .config import EpisodeConfig

#: The entry-point group an application registers its :class:`AppSpec` under.
ENTRY_POINT_GROUP = "freeagent.apps"

#: The entry-point group an application registers its :class:`ManifestSpec`
#: under (ADR-0005). Separate from :data:`ENTRY_POINT_GROUP` precisely so the
#: slim service can resolve an app's manifest metadata **without importing the
#: engine**: a :class:`ManifestSpec` carries only class-ref *strings*, so its
#: defining module imports no engine classes, and resolving it lands no 404 even
#: when the engine is not installed (the worker-pool slim image).
MANIFEST_ENTRY_POINT_GROUP = "freeagent.manifests"


class UnknownAppError(ConfigError):
    """No installed application is registered under the requested REST name."""


@dataclass(frozen=True, slots=True)
class ConfigField:
    """One operator-settable key in a component's ``config`` block.

    Plain, wire-safe data: a field *name*, a JSON-ish *type* tag
    (``"string"`` / ``"integer"`` / ``"number"`` / ``"boolean"``), and a help
    *description*. No class reference, so it survives a trip across a REST
    boundary or into YAML help -- consistent with how
    :class:`~freeagent.cli.config.ComponentSpec` carries only a ``config`` dict.
    """

    name: str
    type: str = "string"
    description: str = ""


@dataclass(frozen=True, slots=True)
class SettableConfig:
    """The operator-settable ``config`` surface of an app, by component.

    Mirrors :class:`~freeagent.cli.config.EpisodeConfig`: :attr:`environment`
    lists the fields settable on the environment, and :attr:`agents` maps each
    roster name to the fields settable on that agent. Everything is
    :class:`ConfigField` data -- no class references on the wire or in YAML.
    """

    environment: tuple[ConfigField, ...] = ()
    agents: Mapping[str, tuple[ConfigField, ...]] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class ManifestSpec:
    """An app's launch identity as **class-ref strings** -- importing no engine.

    The recruiter (ADR-0005) needs only three facts to build an episode's
    manifest set: the subject-prefix :attr:`name`, the environment's
    ``module:QualName`` :attr:`environment` reference, and the
    :attr:`roster` mapping each agent name to its ``module:QualName`` reference.
    Every value here is a *string* -- never a class object -- so a
    :class:`ManifestSpec` is wire-safe and, crucially, its defining module imports
    no engine code. The slim worker-pool service resolves it (see
    :func:`load_manifest_spec`) to enqueue manifests **without** the engine
    installed; only the fat worker that later runs a manifest imports the class
    the string names.

    A fat caller that already holds an :class:`AppSpec` (with live classes) builds
    the equivalent spec via :meth:`AppSpec.manifest_spec`; an app advertises one
    directly on the :data:`MANIFEST_ENTRY_POINT_GROUP` entry point so a slim
    service never has to load the :class:`AppSpec`.
    """

    #: The subject prefix every episode of this app lives under (undashed).
    name: str
    #: ``module:QualName`` reference to the environment class.
    environment: str
    #: Agent name -> ``module:QualName`` reference to that agent's class.
    roster: Mapping[str, str]


@dataclass(frozen=True, slots=True)
class AppSpec:
    """How an application advertises itself to a generic launcher.

    Carries everything :func:`freeagent.run_episode` requires -- the
    subject-prefix :attr:`name`, the :attr:`environment` class, and the
    :attr:`roster` (agent name -> agent class) -- plus the
    :attr:`settable_config` surface an API may expose and the :attr:`cli` Typer
    sub-app the root CLI mounts. An app constructs one in source and registers
    it on the ``freeagent.apps`` entry point; :func:`load_app` hands it back to
    a generic caller keyed by REST name.
    """

    #: The subject prefix every episode of this app lives under (e.g.
    #: ``"twentyquestions"``). Differs from the dashed REST/entry-point name.
    name: str
    environment: type[Environment]
    #: Agent name -> agent class; the names an operator may override in YAML.
    roster: Mapping[str, type[Agent]]
    #: Which ``config`` fields an operator may set, by component.
    settable_config: SettableConfig = field(default_factory=SettableConfig)
    #: The Typer sub-app mounted under the REST name; ``None`` for apps that
    #: are launchable but expose no CLI of their own.
    cli: typer.Typer | None = None

    def __post_init__(self) -> None:
        """Reject a settable surface that names agents outside the roster.

        A generic caller reads :attr:`settable_config` to know which ``config``
        fields an operator may set per agent; if it named an agent the roster
        does not contain, that drift would surface only downstream (a form field
        for a non-existent agent). A roster member with no settable fields may be
        omitted, so this is a subset check, not equality. Validating here makes a
        mismatch a registration-time error -- exactly when the app author can fix
        it -- rather than a silent inconsistency the control service inherits.
        """
        unknown = set(self.settable_config.agents) - set(self.roster)
        if unknown:
            raise ValueError(
                f"settable_config names agent(s) {sorted(unknown)} absent from the "
                f"roster {sorted(self.roster)}"
            )

    def manifest_spec(self) -> ManifestSpec:
        """Project this spec's live classes onto an engine-free :class:`ManifestSpec`.

        The fat path's bridge to the recruiter: a caller holding real classes
        (the CLI, a test) derives the same class-ref strings a slim service reads
        from the :data:`MANIFEST_ENTRY_POINT_GROUP` entry point. Building manifests
        from the result means the recruiter never touches a class object.
        """
        return ManifestSpec(
            name=self.name,
            environment=class_ref(self.environment),
            roster={name: class_ref(cls) for name, cls in self.roster.items()},
        )

    def run(self, config: EpisodeConfig, *, parquet_log: Path | None = None) -> int:
        """Launch and supervise one episode of this app; return its exit code.

        The generic front door onto :func:`freeagent.run_episode`: it supplies
        the app's own name, environment, and roster, so a caller only passes the
        per-episode *config* (and an optional *parquet_log* to record).
        """
        return _run_episode(
            config,
            app=self.name,
            environment=self.environment,
            agents=self.roster,
            parquet_log=parquet_log,
        )


# ----------------------------------------------------------------------
# Reusable config-field descriptions for the framework's own base classes.
# An app composes these with its own keys instead of re-declaring framework
# config; the descriptions live once, next to the keys they describe.
# ----------------------------------------------------------------------

#: Settable config understood by every :class:`freeagent.Agent`.
AGENT_FIELDS: tuple[ConfigField, ...] = (
    ConfigField("grace_period", "number", "seconds of wind-down after shutdown (default 5)"),
)

#: Settable config understood by every :class:`freeagent.LLMAgent` (extends
#: :data:`AGENT_FIELDS`).
LLM_AGENT_FIELDS: tuple[ConfigField, ...] = (
    *AGENT_FIELDS,
    ConfigField("model", "string", "litellm model string; 'fake'/'fake:<path>' picks the fake LLM"),
    ConfigField("system_prompt", "string", "override the agent's system prompt"),
    ConfigField(
        "llm_telemetry",
        "boolean",
        "publish each LLM call's record to the agent's log-only subject (default true)",
    ),
    ConfigField(
        "nudge_interval", "number", "seconds between silence-breaking nudges (off by default)"
    ),
)

#: Settable config understood by every :class:`freeagent.Environment`.
ENVIRONMENT_FIELDS: tuple[ConfigField, ...] = (
    ConfigField("setup_timeout", "number", "seconds to wait for every agent to join (default 30)"),
    ConfigField("episode_timeout", "number", "seconds before the episode times out (default 600)"),
    ConfigField("grace_period", "number", "seconds of wind-down after shutdown (default 5)"),
)


def load_apps() -> dict[str, AppSpec]:
    """Discover every installed application, keyed by REST (entry-point) name.

    This *is* the canonical REST-name -> subject-prefix mapping: each key is the
    dashed entry-point name a generic caller uses, and each value's
    :attr:`AppSpec.name` is the undashed subject prefix. An entry point that
    does not resolve to an :class:`AppSpec` is a packaging error and raises
    :class:`TypeError`.

    Deliberately uncached: each call re-scans installed-distribution metadata, so
    an app installed into a long-running process is picked up without a restart.
    The scan is cheap for the CLI's single dispatch; a hot-path caller (a control
    service launching many episodes) should call this once and hold the result --
    or cache :func:`load_app` itself -- rather than have the library pin a
    snapshot of what is installed.
    """
    apps: dict[str, AppSpec] = {}
    for entry in entry_points(group=ENTRY_POINT_GROUP):
        spec = entry.load()
        if not isinstance(spec, AppSpec):
            raise TypeError(
                f"{ENTRY_POINT_GROUP} entry point {entry.name!r} "
                f"({entry.value}) is not an AppSpec instance"
            )
        apps[entry.name] = spec
    return apps


def load_app(name: str) -> AppSpec:
    """Return the :class:`AppSpec` registered under REST *name*.

    Raises :class:`UnknownAppError` -- a clean, listing error, not a
    ``KeyError`` -- when no installed application matches.
    """
    apps = load_apps()
    try:
        return apps[name]
    except KeyError:
        known = ", ".join(sorted(apps)) or "(none installed)"
        raise UnknownAppError(
            f"unknown application {name!r}; installed applications: {known}"
        ) from None


def load_manifest_specs() -> dict[str, ManifestSpec]:
    """Discover every app's :class:`ManifestSpec`, keyed by REST name -- no engine import.

    Reads the :data:`MANIFEST_ENTRY_POINT_GROUP` group: every value is a
    :class:`ManifestSpec` whose defining module carries only class-ref *strings*,
    so resolving the set imports **no engine code**. This is the slim
    worker-pool service's window onto what it can provision: it learns each app's
    subject prefix, environment ref, and roster refs without the engine installed.

    An entry point that does not resolve to a :class:`ManifestSpec` is a
    packaging error and raises :class:`TypeError`. Uncached, like
    :func:`load_apps`, so a newly-installed app is picked up without a restart.
    """
    specs: dict[str, ManifestSpec] = {}
    for entry in entry_points(group=MANIFEST_ENTRY_POINT_GROUP):
        spec = entry.load()
        if not isinstance(spec, ManifestSpec):
            raise TypeError(
                f"{MANIFEST_ENTRY_POINT_GROUP} entry point {entry.name!r} "
                f"({entry.value}) is not a ManifestSpec instance"
            )
        specs[entry.name] = spec
    return specs


def load_manifest_spec(name: str) -> ManifestSpec:
    """Return the :class:`ManifestSpec` registered under REST *name* -- no engine import.

    The slim service's ``create`` front door (ADR-0005): it resolves an app's
    launch identity as strings and enqueues manifests without importing the
    engine. Raises :class:`UnknownAppError` -- the same clean listing error
    :func:`load_app` raises -- when no installed application matches.
    """
    specs = load_manifest_specs()
    try:
        return specs[name]
    except KeyError:
        known = ", ".join(sorted(specs)) or "(none installed)"
        raise UnknownAppError(
            f"unknown application {name!r}; installed applications: {known}"
        ) from None
