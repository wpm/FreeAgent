# freeagent-sdk

Library for building Free Agent applications.

A Free Agent application is a collection of independent agent processes that
communicate with each other over NATS. This package provides the base classes
those processes are built from.

## Making your application launchable

Your application lives in its own repository and depends only on `freeagent-sdk`.
This guide makes `uv run <your-app>` do the whole dance — bring up the platform
(NATS and the API), build and serve your viewer, and tear the session down on
Ctrl-C — without you writing any orchestration code. The SDK's shared harness
(`freeagent.sdk.run`) owns every spawn and teardown; you only *describe* your
serving stack.

The design behind this is
[ADR-0009: One-command app launch](../../docs/decision-history/0009-one-command-app-launch.md);
its companion diagram
[`docs/launch-process-map.html`](../../docs/launch-process-map.html) shows who
starts what, who owns what, and how the processes talk. The in-repo `collatz`
application is a complete worked example.

### 1. Depend on the SDK, and add `freeagent-api` to dev dependencies

Your application depends on `freeagent-sdk` at runtime. It also needs
`freeagent-api` available **in the environment**, because bringing the platform
up spawns the `freeagent-api` console script — the harness fails with a clear
message telling you to add it if it is missing. The API is a development-time
platform process, not something your distributed wheel should force on its users,
so it belongs in a dev dependency group:

```toml
# pyproject.toml
[project]
name = "acme-werewolf"
dependencies = [
    "freeagent-sdk",
]

[dependency-groups]
dev = [
    "freeagent-api",
]
```

### 2. Register your application's engine (ADR-0006)

The platform turns an application *name* arriving over REST into loadable code
through the `freeagent.applications` entry-point group
([ADR-0006](../../docs/decision-history/0006-entry-point-application-loading.md)).
Register your engine object — the one implementing the SDK's `Application`
protocol — under a bare name:

```toml
# pyproject.toml
[project.entry-points."freeagent.applications"]
werewolf = "acme.werewolf:application"
```

This is the *loading* half (how the worker builds your episode's entities). The
rest of this guide is the *launching* half (how the serving stack comes up
around it); the two are independent.

### 3. Write a `Launcher`

A `Launcher` describes your serving processes to the harness. It is a plain
object — any object with a `name` string and a `services()` method qualifies,
because `Launcher` is a runtime-checkable `Protocol`; you need not inherit from
anything.

Each `Service` is one long-running `serve` process, plus any `prepare` commands
that must run to completion first (a viewer build, say). The harness runs every
`prepare` step before it spawns any `serve`, so a failing build aborts the
session before a single server starts.

```python
# src/acme/werewolf/launch.py
from __future__ import annotations

from pathlib import Path

from freeagent.sdk import Launcher, Service

# The built viewer directory, relative to this file for an in-repo checkout.
_VIEWER_DIR = Path(__file__).resolve().parent / "viewer"


class WerewolfLauncher:
    """Describes the Werewolf serving stack: a single viewer service."""

    name: str = "werewolf"

    def services(self) -> list[Service]:
        return [
            Service(
                name="viewer",
                # `serve` is the long-running process's argv. It must be a list
                # of strings — a bare string would be split into one-character
                # arguments by the subprocess layer.
                serve=["npm", "run", "serve"],
                # `prepare` steps run to completion, in order, before any serve
                # is spawned. Each is its own argv list.
                prepare=[["npm", "install"], ["npm", "run", "build"]],
                # Working directory for this service's prepare and serve commands.
                cwd=_VIEWER_DIR,
                # Printed once the whole stack is up.
                url="http://localhost:8080",
            )
        ]
```

`Service` is a frozen dataclass; only `name` and `serve` are required. `prepare`
defaults to no steps, `cwd` to the session's own directory, and `url` to nothing
printed.

A launcher describes serving processes only — it never creates episodes. Episode
creation stays with clients (your viewer calls the API, which drives the worker),
which is what keeps the API from ever having to import application or worker code.

### 4. Add the console script

Declare a console script whose `main()` hands your launcher to
`freeagent.sdk.run`. `uv run <name>` resolves any console script installed in the
environment, so the name works in your repo with no central registry.

```toml
# pyproject.toml
[project.scripts]
werewolf = "acme.werewolf.launch:main"
```

```python
# src/acme/werewolf/launch.py  (continued)
import sys

from freeagent.sdk import run
from freeagent.sdk.launch import DockerUnavailableError


def main() -> None:
    """Run the Werewolf session via the SDK harness."""
    try:
        # run() returns a process exit code: the failing prepare step's code, or
        # the first nonzero serve exit seen during teardown, or 0 on a clean
        # Ctrl-C. Propagate it as this process's exit status.
        sys.exit(run(WerewolfLauncher()))
    except DockerUnavailableError as error:
        # Bringing NATS up needs Docker. Render the guidance as one clean line
        # rather than a traceback.
        sys.exit(str(error))
```

`DockerUnavailableError` lives in `freeagent.sdk.launch` (not the top-level
`freeagent.sdk` namespace), so import it from there.

### That's it

With those four pieces in place:

```shell
uv run werewolf
```

ensures NATS and the API are up (starting them if needed), runs your `prepare`
steps, foregrounds your `serve` process, prints your URL, and on **Ctrl-C** tears
down only what it spawned. The platform survives the session, so it is there for
the next `uv run werewolf` — or any other application. It is a session, not a
switch: there is no `uv run werewolf --stop`. The platform's own on/off switch is
`uv run start` / `uv run stop`, and it lives in this repo, not yours.

### Shipping a wheel

When you publish your application as a wheel, two things change from the in-repo
dev loop:

- **A wheel cannot `npm run build`.** Locating `_VIEWER_DIR` relative to the
  source file and running a `prepare` build is the *in-repo* path. A published
  application should build its viewer ahead of time and ship the built assets as
  **package data**, then point `serve` at the installed assets. Treat the
  `prepare` build steps as development-only.
- **`freeagent-api` is a dev dependency,** so a plain wheel install does not
  pull it in. That is intentional: the API is a platform process for
  development, and the deployed platform provides it. `uv run <app>` in a dev
  checkout has it because your dev group does.
