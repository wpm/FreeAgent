"""Typer command line interface for the Free Agent worker.

The worker runs one application episode from the command line. ``freeagent-worker run
<application>`` resolves the application name to code (:func:`~freeagent.sdk.load_application`),
builds an :class:`~freeagent.sdk.EpisodeSpec` from the remaining options, and runs the episode to
completion via :func:`~freeagent.worker.runner.run_episode`. There is no API involvement yet -- the
episode is driven entirely from these arguments (:doc:`ADR-0006
</decision-history/0006-entry-point-application-loading>`).

The CLI treats ``--config`` as opaque: it parses the JSON (from a literal string or a file) into a
dict and hands it to the application inside the :class:`~freeagent.sdk.EpisodeSpec`, never
inspecting its contents. Every failure -- an unknown application, malformed config, or an episode
that errors or times out -- exits nonzero with a message on stderr, so a caller (a shell, later the
API) can tell a completed episode from a failed one by exit code alone.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import typer
from freeagent.sdk import EpisodeSpec, UnknownApplication, available_applications, load_application
from freeagent.sdk.entity import DEFAULT_NATS_SERVER
from freeagent.worker.runner import run_episode

# `no_args_is_help` shows usage instead of an error when invoked bare; the explicit callback keeps
# Typer in multi-command mode so the subcommand is spelled `freeagent-worker run ...` rather than
# collapsing to a single top-level command (Typer drops the subcommand name when there is only one).
SERVERS_CONFIG_KEY = "servers"
"""The config key the worker pins ``--nats-url`` into so entities and the observer share a server.

The SDK's :class:`~freeagent.sdk.entity.Entity` takes its NATS server(s) under this name, and an
application's config carries the same key through to the entities it builds (e.g. Collatz's
``CollatzConfig.servers``). The worker owns connectivity, so it makes ``--nats-url`` authoritative
by writing it here; this is the one config key the otherwise-opaque config is allowed to touch.
"""

app = typer.Typer(help="Free Agent worker: run an application episode.", no_args_is_help=True)


@app.callback()
def main() -> None:
    """Free Agent worker: run an application episode from the command line."""


def _load_config(config: str | None) -> dict[str, Any]:
    """Parse the ``--config`` option into an application config dict.

    The value is JSON, given either as a literal string or as the path to a file containing it; a
    path that exists is read, otherwise the value is parsed as a literal. An omitted ``--config`` is
    an empty config. The parsed JSON must be a JSON object, since an application's config is a dict.

    :param config: The raw ``--config`` value: a JSON literal, a path to a JSON file, or ``None``.
    :return: The parsed config dict, opaque to the worker.
    :raises ValueError: If the value isn't valid JSON, or is valid JSON that isn't an object.
    """
    if config is None:
        return {}
    candidate = Path(config)
    text = candidate.read_text() if candidate.is_file() else config
    parsed = json.loads(text)
    if not isinstance(parsed, dict):
        raise ValueError(f"config must be a JSON object, got {type(parsed).__name__}")
    return parsed


@app.command()
def run(
    application: str = typer.Argument(..., help="Name of the application to run, e.g. `collatz`."),
    episode_id: str = typer.Option(..., "--episode-id", help="Identifier for this episode."),
    nats_url: str = typer.Option(
        DEFAULT_NATS_SERVER,
        "--nats-url",
        help="NATS server URL for the whole episode; pinned into config so entities and the "
        "observer share it.",
    ),
    config: str | None = typer.Option(
        None, "--config", help="Application config as a JSON string or a path to a JSON file."
    ),
    episode_root: str | None = typer.Option(
        None,
        "--episode-root",
        help="Root NATS subject for the episode; defaults to `episode.{episode-id}`.",
    ),
    timeout: float | None = typer.Option(
        None, "--timeout", help="Seconds to wait for the episode to complete; unset waits forever."
    ),
) -> None:
    """Run one episode of ``application`` to completion, exiting nonzero on any failure.

    Resolves ``application`` to code, parses ``--config``, builds the
    :class:`~freeagent.sdk.EpisodeSpec`, and runs the episode. An unknown application name exits
    with a message listing the applications that *are* installed; any other failure exits with the
    error.

    :param application: The bare application name to look up among installed applications.
    :param episode_id: The episode's identifier, unique within a deployment.
    :param nats_url: The NATS server URL for the whole episode; pinned into ``config`` under
        :data:`SERVERS_CONFIG_KEY` so the application's entities and the observer share one server.
    :param config: The application config, as a JSON string or a path to a JSON file.
    :param episode_root: The root NATS subject; defaults to ``episode.{episode_id}``.
    :param timeout: Seconds to wait for completion, or ``None`` to wait indefinitely.
    """
    try:
        loaded = load_application(application)
    except UnknownApplication:
        installed = ", ".join(sorted(available_applications())) or "(none installed)"
        typer.echo(
            f'Unknown application "{application}". Available applications: {installed}', err=True
        )
        raise typer.Exit(code=1) from None

    try:
        parsed_config = _load_config(config)
    except (OSError, ValueError) as error:
        typer.echo(f"Could not read config: {error}", err=True)
        raise typer.Exit(code=1) from None

    # `--nats-url` is the one server the whole episode uses. Pin it into the config under the
    # platform-wide ``servers`` key so the application builds its entities on the same server the
    # worker's observer watches; otherwise an application defaulting ``servers`` elsewhere would run
    # the episode on a different server and the observer would never see EpisodeComplete. The key is
    # the SDK convention (see freeagent.sdk.entity); an application that ignores it is unaffected.
    parsed_config[SERVERS_CONFIG_KEY] = nats_url

    spec = EpisodeSpec(
        episode_root=episode_root or f"episode.{episode_id}",
        episode_id=episode_id,
        config=parsed_config,
    )

    try:
        asyncio.run(run_episode(loaded, spec, nats_url, timeout=timeout))
    except Exception as error:
        typer.echo(f"Episode failed: {error}", err=True)
        raise typer.Exit(code=1) from None

    typer.echo(f'Episode "{episode_id}" of application "{application}" complete.')


if __name__ == "__main__":
    app()
