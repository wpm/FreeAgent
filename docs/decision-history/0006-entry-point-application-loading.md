# ADR-0006: Entry-point application loading

**Status:** Proposed
**Date:** 2026-07-02
**Deciders:** Bill McNeill

> First ADR of the `rewrite` branch. The rewrite rebuilds Free Agent bottom-up:
> `freeagent-sdk` (entities over NATS) exists; `freeagent-worker` and
> `freeagent-api` come next. This ADR supersedes the *manifest package*
> mechanism of [ADR-0002](0002-control-plane-service.md)/[ADR-0004](0004-app-independent-service.md)
> (`AppSpec`, per-app manifest distributions) as the way applications are
> registered and found.

## Context

A Free Agent *application* (Twenty Questions, Werewolf, the forthcoming
Collatz test app) supplies `Agent` and `Environment` subclasses built on the
SDK. The API must be able to create an episode from a request like
*(application, episode ID, number of players)*, and the worker must turn that
into running entity processes. So somewhere there is a mapping from an
application **name** arriving over REST to application **code** loadable with
`import`. The question is where that mapping lives and how entries get into it.

Simplifying assumptions for this phase, accepted deliberately:

- Application code is always installed in the same Docker image as the worker
  (and, for now, the API).
- One image, one machine; distribution concerns are out of scope
  (see [ADR-0005](0005-the-worker-pool.md) for where they went last time).

The previous incarnation solved this with separate manifest packages that
advertised an app without importing it. That worked but carried real overhead:
an extra distribution per app to author, version, and keep in sync with the
engine it described. The rewrite wants the cheapest mechanism that still gives
runtime lookup by name and enumeration of what is installed.

## Decision

**Applications register themselves as Python entry points in the group
`freeagent.applications`. The registry is pip's installed-distribution
metadata; there is no Free Agent registry code beyond a lookup helper.**

Each application declares, in its own `pyproject.toml`:

```toml
[project.entry-points."freeagent.applications"]
collatz = "freeagent.app.collatz:application"
```

The SDK provides the loader (approximately):

```python
from importlib.metadata import entry_points

def load_application(name: str) -> Application:
    (ep,) = entry_points(group="freeagent.applications", name=name)
    return ep.load()

def available_applications() -> list[str]:
    return [ep.name for ep in entry_points(group="freeagent.applications")]
```

"Registered" means "pip-installed in the image", nothing more. Adding an
application to a deployment is `pip install freeagent-app-werewolf`; removing
it is uninstalling. Registration is declarative install-time metadata, so
there is no import-order chicken-and-egg and no import-time side effect needed
to populate the registry.

### Naming

Application names are **bare** (`collatz`, `twenty_questions`), not dotted
module paths. The entry-point *group* is the platform namespace: a lookup in
`freeagent.applications` can only ever see Free Agent applications, so a
`freeagent.` prefix on each name would restate what the group already
enforces while resembling an import path the name is deliberately decoupled
from. If third-party applications ever share a deployment, the collision risk
is between *authors*, not platforms; the convention then is author-qualified
names (`acme.werewolf`), which remain opaque identifiers.

### What an entry point resolves to

The entry point resolves to an object satisfying an `Application` protocol,
defined in the SDK:

```python
class Application(Protocol):
    name: str

    def make_environment(self, episode: EpisodeSpec) -> Environment: ...
    def make_agents(self, episode: EpisodeSpec) -> list[Agent]: ...
```

`EpisodeSpec` is the SDK-level episode description: `episode_root` (the NATS
subject root), `episode_id`, and a `config: dict[str, Any]` whose contents are
**app-defined**. The API and worker treat `config` as opaque; an application
validates it with its own pydantic model inside `make_*` (the same
Python-as-source-of-truth discipline as ADR-0007's message schemas). This
keeps the platform surface fixed — *(application, episode ID, config)* — while
letting Werewolf take role assignments and Collatz take a starting integer
without either leaking into SDK types. `# Players` from the motivating example
is just a `config` key.

The worker stays a dumb host: resolve name → build entities from the spec →
run them. All application intelligence lives behind the protocol.

## Options considered

### Option A: Manifest packages (status quo ante, ADR-0002/0004)

Separate per-app distributions advertising `AppSpec` without importing the app.

**Pros:** advertisement without import; slim service image possible.
**Cons:** an entire extra package per app to author and keep in sync; the
complexity that motivated the rewrite. Solves an image-separation problem this
phase has explicitly assumed away.

### Option B: Decorator registry (`@register_application`)

A module-level dict populated at import time.

**Pros:** no packaging metadata; plain Python.
**Cons:** registration requires the module to have been imported first, so a
second mechanism must import all app modules — the original problem, unsolved.

### Option C: Convention — name *is* the module path

`importlib.import_module("freeagent.app.twenty_questions")` directly.

**Pros:** zero registry of any kind.
**Cons:** couples the public API surface to internal module layout; imports a
caller-supplied string; no enumeration of installed apps without scanning a
namespace package.

### Option D: Config file mapping names to module paths

**Pros:** explicit, inspectable.
**Cons:** one more artifact that can drift from what is actually installed;
entry points cannot drift by construction.

### Option E: Entry points (chosen)

**Pros:** standard Python plugin mechanism (pytest, Flake8); declarative;
enumerable; name decoupled from module layout; registry maintained by pip.
**Cons:** registration requires an installed distribution — apps cannot be
loaded from a bare directory of source files. Acceptable: apps are workspace
members and `pip install -e` covers development.

## Consequences

- Easier: adding an application is one `pyproject.toml` stanza and an install.
  The worker and API share one trivial lookup path.
- Easier: `available_applications()` gives the API a listing/validation
  endpoint for free.
- Harder: nothing runs from un-installed source; development requires editable
  installs (already the uv-workspace practice).
- Deferred: separating the API's image from app images reopens the
  advertisement-without-import question; that future ADR can shrink what the
  API loads (see ADR-0007, which removes the API's need to load app code at
  all) rather than resurrect manifests.
- The `Application` protocol is the worker's entire knowledge of any app;
  changes to it are platform-breaking and deserve their own ADRs.

## Action items

1. [ ] Add `Application` protocol and `EpisodeSpec` to `freeagent-sdk`.
2. [ ] Add `load_application` / `available_applications` helpers with tests
       (a test-only entry point registered via a metadata fixture).
3. [ ] Create `apps/` uv-workspace with `apps/collatz` as first member,
       registering the `collatz` entry point from day one.
4. [ ] Wire `freeagent-worker` to resolve and run an application by name.
