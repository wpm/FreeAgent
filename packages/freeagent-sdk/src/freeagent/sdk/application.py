"""Applications: named bundles of :class:`~freeagent.sdk.entity.Agent` and
:class:`~freeagent.sdk.entity.Environment` code, found by name at runtime.

A Free Agent *application* (Twenty Questions, Werewolf, the Collatz test app) supplies the
:class:`~freeagent.sdk.entity.Agent` and :class:`~freeagent.sdk.entity.Environment` subclasses that
make up an episode. The platform — the API and the worker — knows an application only by a bare
name arriving over REST (``collatz``, ``twenty_questions``); it must turn that name into loadable
code. Applications
register themselves as Python *entry points* in the group ``freeagent.applications``:

.. code-block:: toml

    [project.entry-points."freeagent.applications"]
    collatz = "freeagent.app.collatz:application"

The registry is pip's installed-distribution metadata; there is no Free Agent registry code beyond
the two helpers here. :func:`load_application` resolves a name to the registered
:class:`Application` object; :func:`available_applications` enumerates every installed application's
name. An application's runtime knobs — Werewolf's role assignments, Collatz's starting integer —
travel in an :class:`EpisodeSpec`'s ``config`` dict, opaque to the platform and validated by the
application itself inside :meth:`Application.make_environment` / :meth:`Application.make_agents`.
"""

from __future__ import annotations

from importlib.metadata import entry_points
from typing import Any, Protocol, runtime_checkable

from freeagent.sdk.entity import Agent, Environment
from pydantic import BaseModel

APPLICATION_ENTRY_POINT_GROUP = "freeagent.applications"
"""The entry-point group applications register themselves in; see :func:`load_application`."""


class UnknownApplication(LookupError):
    """Raised when :func:`load_application` is asked for an application name that isn't installed.

    A subclass of :class:`LookupError` so the name-not-found case is catchable in isolation — the
    API turns it into a 404 — without also swallowing errors raised while *importing* an
    application that does exist.
    """


class AmbiguousApplication(LookupError):
    """Raised when a name resolves to more than one installed application.

    Entry points aren't deduplicated across distributions, so two installed distributions can each
    register the ``freeagent.applications`` group under the same name — a collision possible
    between third-party authors. Rather than let the single-match unpack in
    :func:`load_application` fail with a bare, uncatchable :class:`ValueError`, that case raises
    this. A :class:`LookupError` like :class:`UnknownApplication`, so a caller can catch both
    resolution failures at once, while their distinct types let the API tell "no such application"
    (a 404) apart from "ambiguous install" (an operator misconfiguration).
    """


class InvalidApplication(TypeError):
    """Raised when a name resolves to an object that isn't a valid :class:`Application`.

    An entry point can point at anything importable; :func:`load_application` checks that what it
    loads actually satisfies the :class:`Application` protocol (has ``name`` and both factory
    methods) and raises this if it doesn't, so a misregistered entry point fails loudly at load
    time — naming the offending application — rather than deep inside the worker with an opaque
    :class:`AttributeError` when a missing method is finally called. A :class:`TypeError`, not a
    :class:`LookupError`: the application was *found*, it's just the wrong shape.
    """


class EpisodeSpec(BaseModel):
    """The platform-level description of one episode, handed to an :class:`Application` to build
    from.

    This is the whole surface the platform pins down: which NATS subject tree the episode lives
    under, its identifier, and an application-defined ``config``. Everything an individual
    application needs beyond that travels inside ``config``, which the API and worker treat as
    opaque; an application validates it with its own pydantic model inside
    :meth:`Application.make_environment` / :meth:`Application.make_agents`.

    :ivar episode_root: The root NATS subject the episode's entities communicate under.
    :ivar episode_id: The episode's identifier, unique within a deployment.
    :ivar config: Application-defined settings, opaque to platform code. An application validates
        this itself; the platform never inspects it.
    """

    episode_root: str
    episode_id: str
    config: dict[str, Any]


@runtime_checkable
class Application(Protocol):
    """The platform's entire knowledge of an application: a name and two factories.

    An entry point in the ``freeagent.applications`` group resolves to an object satisfying this
    protocol (see :func:`load_application`). Given an :class:`EpisodeSpec`, the object builds the
    one :class:`~freeagent.sdk.entity.Environment` and the :class:`~freeagent.sdk.entity.Agent`
    instances that make up an episode; the worker then runs them. All application intelligence
    lives behind this protocol, so the worker stays a dumb host.

    Because it is a :class:`~typing.Protocol`, an application need not inherit from anything — any
    object with a matching ``name`` and the two methods qualifies. It is also
    :func:`~typing.runtime_checkable`, so ``isinstance(obj, Application)`` checks the shape at
    runtime (structurally: it confirms the attributes exist, not their signatures).

    :ivar name: The application's bare name, matching the entry point it registered under.
    """

    name: str

    def make_environment(self, episode: EpisodeSpec) -> Environment:
        """Build the single :class:`~freeagent.sdk.entity.Environment` for an episode.

        :param episode: The episode to build for; validate any application-specific settings out of
            its ``config`` here.
        :return: The episode's environment, not yet started.
        """
        ...

    def make_agents(self, episode: EpisodeSpec) -> list[Agent]:
        """Build the :class:`~freeagent.sdk.entity.Agent` instances for an episode.

        :param episode: The episode to build for; validate any application-specific settings out of
            its ``config`` here.
        :return: The episode's agents, not yet started.
        """
        ...


def load_application(name: str) -> Application:
    """Resolve an application name to the :class:`Application` object registered under it.

    Looks the name up among the ``freeagent.applications`` entry points and loads the object it
    points at — the same object the application named in its ``pyproject.toml``. "Registered" means
    "pip-installed in this environment": an application the current environment doesn't have
    installed is unknown here. The loaded object is checked against the :class:`Application`
    protocol before it's returned, so a misregistered entry point fails here rather than downstream.

    :param name: The application's bare name, e.g. ``collatz``.
    :return: The registered :class:`Application` object.
    :raises UnknownApplication: If no installed application registered under ``name``.
    :raises AmbiguousApplication: If more than one installed application registered under ``name``.
    :raises InvalidApplication: If ``name`` resolves to an object that isn't a valid
        :class:`Application`.
    """
    matches = entry_points(group=APPLICATION_ENTRY_POINT_GROUP, name=name)
    if not matches:
        raise UnknownApplication(f'No application named "{name}" is installed')
    if len(matches) > 1:
        sources = ", ".join(sorted(entry_point.value for entry_point in matches))
        raise AmbiguousApplication(
            f'Application name "{name}" is registered by more than one distribution: {sources}'
        )
    (entry_point,) = matches
    application = entry_point.load()
    if not isinstance(application, Application):
        raise InvalidApplication(
            f'Application "{name}" ({entry_point.value}) does not satisfy the Application protocol'
        )
    return application


def available_applications() -> list[str]:
    """List the names of every application installed in this environment.

    Enumerates the ``freeagent.applications`` entry-point group; each name is one loadable with
    :func:`load_application`. The order follows :func:`~importlib.metadata.entry_points` and is not
    guaranteed.

    :return: The bare name of each installed application.
    """
    return [entry_point.name for entry_point in entry_points(group=APPLICATION_ENTRY_POINT_GROUP)]
