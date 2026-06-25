"""The episode manifest: the serializable unit of work a worker pulls (ADR-0005).

ADR-0005 promotes today's child spec (``cli/child.py``'s ``{role, class,
subject_root, agent_id, config, nats_url}``) into a first-class, versioned,
**wire-safe manifest** -- the unit of work a worker pulls off the JetStream
work queue. A :class:`Manifest` *references* code by import path
(``module:QualName``); it does **not** ship code. A worker that runs a manifest
must therefore have the named engine installed (workers are "fat").

The manifest is single-sourced from this Pydantic model and exported through the
``pnpm run schemas`` pipeline (JSON Schema + generated TS types), exactly as the
episode contract is (ADR-0001): it is added to
:data:`freeagent.schema.FRAMEWORK_MODELS`.

A manifest is the *one* shape for every launchable role. The two child shapes
``child.py`` already knows -- agent and environment -- become two roles of one
model: an ``agent`` carries :attr:`Manifest.subject_root` + :attr:`agent_id`; an
``environment`` carries :attr:`app` + :attr:`roster` + :attr:`episode_id`.
:meth:`Manifest.to_child_spec` produces exactly the dict
:func:`freeagent.cli.child.run_spec` already consumes, so a child is launchable
purely from a manifest with **no behaviour change**.

A manifest also records a **resolved engine version** (e.g.
``twentyquestions==1.2.3``) as write-only provenance, stamped once the child
imports the class (:meth:`Manifest.stamp_resolved_version`). It gates nothing in
v1 -- it is the difference between an RL trajectory you can reproduce and one you
cannot, consistent with "the wire is the log."
"""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError, version
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

#: The manifest schema version. Bumped when the wire shape changes
#: incompatibly, so a worker can tell a manifest it cannot launch from one it
#: can. It gates nothing in v1, but it is the hook for that future check.
MANIFEST_VERSION = 1

#: The launchable roles. Mirrors the roles ``child.py`` dispatches on; the
#: recorder is launched by its own argv contract, not as a manifest role.
Role = Literal["agent", "environment"]


class Manifest(BaseModel):
    """A serializable, versioned unit of work: one launchable role of one episode.

    Wire-safe (round-trips JSON) and self-contained: a child is launchable purely
    from a manifest via :meth:`to_child_spec`. References code by
    :attr:`cls` import path; never carries code.

    Role-specific fields are validated by :meth:`_check_role_fields`: an
    ``agent`` requires :attr:`subject_root` and :attr:`agent_id`; an
    ``environment`` requires :attr:`app`, :attr:`roster`, and :attr:`episode_id`.
    """

    # ``extra="forbid"`` so a stray wire field is a loud error, not silent
    # drift; ``populate_by_name`` so the model is constructible in Python by the
    # field name ``cls`` and on the wire by its ``class`` alias.
    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    role: Role
    #: ``module:QualName`` reference the child imports and runs. Aliased to
    #: ``class`` on the wire (``class`` is a Python keyword, so the attribute is
    #: ``cls``); the JSON the worker and child exchange uses ``class``, matching
    #: today's child spec.
    cls: str = Field(alias="class")
    #: Verbatim constructor kwargs for the referenced class.
    config: dict[str, Any] = Field(default_factory=dict)
    nats_url: str
    #: The manifest schema version (see :data:`MANIFEST_VERSION`).
    manifest_version: int = MANIFEST_VERSION

    # -- agent-role fields --
    #: ``<app>.episode.<id>`` the agent runs under (agent role only).
    subject_root: str | None = None
    #: The agent's id within the roster (agent role only).
    agent_id: str | None = None

    # -- environment-role fields --
    #: The undashed subject prefix (environment role only).
    app: str | None = None
    #: The roster agent names the environment expects (environment role only).
    roster: list[str] | None = None
    #: The episode id the environment creates (environment role only).
    episode_id: str | None = None

    #: Resolved engine version stamped after import (e.g. ``freeagent==1.0.0``).
    #: Write-only provenance; gates nothing in v1. ``None`` until stamped.
    resolved_version: str | None = None

    def model_dump_json(self, **kwargs: Any) -> str:
        """Serialize to JSON with the ``class`` alias on the wire by default.

        The wire shape must carry ``class`` (matching today's child spec), not
        the Python attribute name ``cls``. An explicit ``by_alias`` still wins.
        """
        kwargs.setdefault("by_alias", True)
        return super().model_dump_json(**kwargs)

    def model_dump(self, **kwargs: Any) -> dict[str, Any]:
        """Serialize to a dict with the ``class`` alias by default (see :meth:`model_dump_json`)."""
        kwargs.setdefault("by_alias", True)
        return super().model_dump(**kwargs)

    @model_validator(mode="after")
    def _check_role_fields(self) -> Manifest:
        """Require the fields a role's child constructor needs, reject the rest."""
        if self.role == "agent":
            missing = [name for name in ("subject_root", "agent_id") if getattr(self, name) is None]
            if missing:
                raise ValueError(f"agent manifest is missing required field(s): {missing}")
        elif self.role == "environment":
            missing = [
                name for name in ("app", "roster", "episode_id") if getattr(self, name) is None
            ]
            if missing:
                raise ValueError(f"environment manifest is missing required field(s): {missing}")
        return self

    def to_child_spec(self) -> dict[str, Any]:
        """The JSON-serializable child spec :func:`run_spec` consumes, unchanged.

        Produces exactly the dict
        :func:`freeagent.cli.child.agent_spec` / ``environment_spec`` build today,
        so a child launched from a manifest behaves identically. Manifest-only
        fields (``manifest_version``, ``resolved_version``) are provenance and do
        not belong in the launch spec, so they are omitted.
        """
        spec: dict[str, Any] = {"role": self.role, "class": self.cls}
        if self.role == "agent":
            spec.update(
                subject_root=self.subject_root,
                agent_id=self.agent_id,
                config=dict(self.config),
                nats_url=self.nats_url,
            )
        else:  # environment
            spec.update(
                app=self.app,
                roster=list(self.roster or []),
                episode_id=self.episode_id,
                config=dict(self.config),
                nats_url=self.nats_url,
            )
        return spec

    def with_resolved_version(self, stamp: str) -> Manifest:
        """A copy of this manifest carrying *stamp* as its resolved version.

        Returns a new manifest rather than mutating in place: the stamp is
        write-once provenance the worker/child records after import, then writes
        into the durable record.
        """
        return self.model_copy(update={"resolved_version": stamp})

    def stamp_resolved_version(self, cls: type) -> Manifest:
        """A copy stamped with the resolved version of the imported *cls*.

        *cls* is the class :attr:`cls` resolved to. The version comes from the
        installed distribution that ships *cls*'s top-level package; a class with
        no installed distribution leaves the stamp ``None`` (see
        :func:`resolved_version_for`).
        """
        return self.with_resolved_version(resolved_version_for(cls))


def resolved_version_for(cls: type) -> str | None:
    """The ``<distribution>==<version>`` of the package shipping *cls*, or ``None``.

    Resolves the installed-distribution version of *cls*'s top-level import
    package (e.g. a class in ``twentyquestions.player`` resolves the
    ``twentyquestions`` distribution to ``twentyquestions==1.2.3``). Returns
    ``None`` when the package has no installed distribution metadata -- a class
    defined ad hoc, in a test, or in a namespace that was never packaged -- so
    stamping a loose class is a no-op rather than an error.
    """
    package = (cls.__module__ or "").split(".", 1)[0]
    if not package:
        return None
    try:
        return f"{package}=={version(package)}"
    except PackageNotFoundError:
        return None
