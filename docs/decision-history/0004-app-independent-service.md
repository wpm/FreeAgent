# ADR-0004: The app-independent episode service

**Status:** Accepted
**Date:** 2026-06-25
**Deciders:** Bill McNeill

## Context

ADR-0003 ("The atemporal episode") made the episode service the sole NATS client
and, to give a browser one process to talk to, folded the **UI into the service**:
the service serves the built UI bundle from its own origin, the Docker image is
built in two stages (a Node stage builds the Twenty Questions viewer, a Python
stage copies the bundle in), and a single launch script brings up "the Twenty
Questions UI" at `http://localhost:8000`.

That "one origin" convenience quietly coupled the framework to one application.
The consequences:

- **The `freeagent` image is not app-independent.** It bakes in the Twenty
  Questions viewer bundle and (via `uv sync` over the whole workspace) the
  `twentyquestions` engine. The image that is supposed to be *substrate* ships a
  specific *application*.
- **"Serve the UI" is an application concern living in the library.** `create_app`
  mounts `StaticFiles`; `serve` grows a `--ui-dir` / `$FREEAGENT_UI_DIR`. None of
  that is about episodes-over-JetStream; it is about hosting one app's front end.
- **The boundary the project actually wants is REST.** FreeAgent's value is a
  stateless REST/JetStream API any client can drive. A UI is just one such client.
  Conflating the two makes "run the backend" and "run a UI" the same act, which is
  exactly backwards for a substrate.

The Twenty Questions viewer is already a self-contained Vite SPA: it resolves its
service URL from configuration, talks only to the REST API plus the per-episode
WebSocket feed, and never speaks NATS. Nothing about it *needs* to be served from
the service's origin; it was merely convenient to do so.

This is still the trusted local testbed of ADR-0001/0002/0003: localhost, no auth,
JetStream on a local volume.

## Decision

**The FreeAgent episode service is app-independent: a REST/JetStream API and
nothing more.** It serves no UI and bundles no application. This supersedes the
"service serves the UI bundle from one origin" decision in ADR-0003 (#41) and the
"freeagent = episode service + UI host" framing of its Docker network (#46).

Concretely:

- **No UI in the service.** Remove the `StaticFiles` mount, the `ui_dir` parameter
  threaded through `create_app` / `run`, and the `--ui-dir` / `$FREEAGENT_UI_DIR`
  option on `serve`. The service exposes only the REST resources and the
  per-episode feed. CORS stays, because the UI is now necessarily cross-origin.

- **No application in the image.** The `freeagent` image installs only the
  `freeagent` library (`uv sync --frozen --package freeagent`); it copies no
  viewer bundle and no app engine sources. The Docker network is the FreeAgent
  **backend** — `nats` (internal) and `freeagent` (the only published port) — not
  "the Twenty Questions UI."

- **A UI is a separate host process.** The Twenty Questions viewer runs on the
  host (`pnpm dev`, its own port — `5173` by default) and calls the REST API
  cross-origin. It is not part of the Docker network. The single
  `docker/twenty-questions.sh` launch script, which conflated bringing up the
  backend with opening "the UI," is removed.

The REST contract, the feed, the JetStream-resident episode model, and Parquet
edge I/O from ADR-0003 are all unchanged. This decision is only about **where the
UI lives and what the image contains** — the seam between front end and back end
is exactly the REST contract ADR-0003 already pinned (#39).

### Not in scope: who launches the engine

ADR-0003's service still **launches the episode engine in-process** (its `create`
endpoint starts the agent child processes inside the `freeagent` container, and
`stop`/status rely on the live handles it holds). That in-process launching means
the image must still carry the app engine's *manifest* to resolve the workspace,
and means the service is not yet purely "runs no app code."

Fully removing engine-launching — making the service pure CRUD and deciding what
process launches and supervises episodes instead — is a **separate decision** with
its own design questions (a launcher process? create-provisions-only with an
external runner? how does stop/liveness work without held handles?). It is
deliberately deferred to a future ADR and tracked as its own issue. This ADR
decouples the **UI and the image**; the engine-launch decoupling follows.

**Transitional consequence (known and accepted).** Because this ADR removes the
app engine from the image *before* the separate decision removes in-process
launching, the slim `freeagent` image temporarily **cannot launch a live
episode**: `create` for an application whose engine is not installed returns
`404 unknown application` (the service still tries to `load_app` in-process, but
the engine is no longer there). The app-agnostic operations — list, get, the
feed, import/export, rename, delete — all work against JetStream-resident
episodes. Launching a live game is restored by the follow-up that moves
engine-launching out of the service. This in-between state is accepted as the
cost of landing the decoupling in two honest steps rather than one large one.

## Options Considered

### Where the UI is served

| Option | Assessment |
|--------|------------|
| **Separate host process; service is REST-only (chosen)** | The library stays substrate; the image is app-independent; "run the backend" and "run a UI" are distinct acts. A UI is just one REST client among many. |
| Service serves the UI bundle (ADR-0003) | One origin for a browser, but bakes one application into the framework image and puts an app-hosting concern in the library. |
| A third "UI" container in the network | Decouples the bundle from the `freeagent` image, but still ties a specific app's deployment to the framework's compose file; a host `pnpm dev`/static serve is simpler and already how the viewer runs in development. |

### What the `freeagent` image installs

| Option | Assessment |
|--------|------------|
| **Only the `freeagent` library (chosen)** | `uv sync --package freeagent` excludes the app engines; the image is the substrate, nothing more. |
| The whole workspace (ADR-0003) | Simple `uv sync`, but pulls every companion app's engine into the framework image. |

## Consequences

- **Easier:** the `freeagent` image is genuinely app-independent and smaller (no
  Node build stage, no viewer bundle, no app engine); the library no longer
  carries UI-hosting code; the REST boundary is the only contract between back end
  and any UI; multiple UIs (or none) can target one backend.
- **Harder / new work:** a UI is now always cross-origin, so it depends on CORS
  being configured (it is) and on the operator launching it separately — there is
  no longer a single "bring up the whole app" command. The READMEs and the quick
  start must spell out the two-step "backend, then UI" flow.
- **Watch / revisit:** the deferred engine-launch decoupling (a future ADR); and,
  as ever, auth and non-local binding if the local-testbed assumption changes.

## Action Items

1. [x] Remove UI serving from the service: `StaticFiles` mount, `ui_dir`,
       `--ui-dir` / `$FREEAGENT_UI_DIR`. Keep CORS.
2. [x] `freeagent` image installs only the library (`--package freeagent`); no
       viewer bundle, no app engine sources; single Python build stage.
3. [x] Docker compose is the FreeAgent **backend** (nats + freeagent); remove
       `docker/twenty-questions.sh` and the "open the UI" framing.
4. [x] Update the READMEs to the two-step flow: run the backend, then run the
       viewer (`pnpm dev`) as a separate host process.
5. [ ] (Separate ADR/issue) Remove in-process engine launching; decide what
       launches and supervises episodes once the service is pure CRUD.
