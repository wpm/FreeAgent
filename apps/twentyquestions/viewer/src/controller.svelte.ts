/**
 * The episode controller: the app's multiplexed control plane over many
 * episodes at once.
 *
 * The original viewer watched a single episode named by hand in a URL. This
 * turns it into a controller: it talks to the control service over REST
 * (`./control`) to **configure and launch** episodes — live or replayed — then
 * watches several **concurrently**, multiplexing the unchanged single-episode
 * `EpisodeViewer` into one live transcript per episode. The viewer side stays
 * exactly as it was: read-only NATS subscription, one code path for live and
 * replay (ADR-0001). All mutation goes over REST; the browser never publishes to
 * NATS.
 *
 * A watched episode is either **control-launched** (this app started it, so it
 * carries the service's `EpisodeView` and can be stopped over REST) or
 * **observed** (attached read-only to a subject named by hand or by a shared
 * URL, exactly as the old viewer did, with no control-plane handle). Both render
 * through the same transcript panel.
 */
import { wsconnect, type NatsConnection } from "@nats-io/nats-core";
import { jetstreamManager, type JetStreamManager } from "@nats-io/jetstream";

import { resolveControllerConfig, type ControllerConfig } from "./config";
import {
  ControlClient,
  publicSubjectOf,
  type ComponentConfig,
  type CreateEpisodeRequest,
  type EpisodeView,
} from "./control";
import { discoverEpisodes, type DiscoveredEpisode } from "./discovery";
import { EpisodeViewer } from "./viewer.svelte";

/** The settable launch surface an operator drives from the browser.
 *
 * The roster (player count) is fixed by the application source for v1, so the
 * only knobs here are the Host's `secret`, a `model` override applied to every
 * LLM agent, and the `parquetLog` "logging file" to record the run to. Blank
 * fields are simply omitted, leaving the app's own defaults in force.
 */
export interface LaunchSettings {
  /** The word or phrase for the Host to defend; blank picks a random canned one. */
  secret: string;
  /** litellm model string applied to the Host and every Player; blank keeps defaults. */
  model: string;
  /** Record the live episode to this Parquet path (must not already exist). */
  parquetLog: string;
  /** Pin the episode id (`[A-Za-z0-9_-]+`); blank lets the service mint one. */
  episodeId: string;
}

/** One episode being watched: its viewer plus, if we launched it, its REST handle. */
export interface WatchedEpisode {
  /** Stable key for `{#each}`, unique across this controller's lifetime. */
  key: string;
  /** The read-only NATS subscriber rendering this episode's transcript. */
  viewer: EpisodeViewer;
  /** "control" when we launched it over REST; "observe" when attached by subject. */
  source: "control" | "observe";
  /** The service's view, for a control-launched episode; null for an observed one. */
  view: EpisodeView | null;
  /** A short human label for the panel header. */
  title: string;
}

/**
 * One discovered episode as the controller surfaces it: the JetStream facts plus
 * the control service's REST view *when it owns the episode*. A null `view` means
 * the service does not know it (CLI-launched, another instance, pre-restart) — it
 * is still watchable, just not stoppable. Where `view` is present its lifecycle
 * `status`/`detail` and `controllable` flag overlay the bare wire facts.
 */
export interface DiscoveredEntry extends DiscoveredEpisode {
  view: EpisodeView | null;
}

/** The Host agent's roster name — where its `secret` config rides. */
const HOST = "host";
/** Every roster member a `model` override applies to (the Host plus the Players). */
const LLM_AGENTS = ["host", "alice", "bob", "carol"] as const;

/** A monotonically increasing source of unique panel keys. */
let nextKey = 0;

export class EpisodeController {
  /** Where the control plane lives and how its episodes are watched. */
  config = $state<ControllerConfig>(resolveControllerConfig());
  /** The episodes currently on screen, in the order they were added. */
  episodes = $state<WatchedEpisode[]>([]);
  /** The last control-plane error, for the operator to see why a launch failed. */
  error = $state("");
  /** True while a control request is in flight, to disable the launch controls. */
  busy = $state(false);
  /** Every episode found on the bus, newest activity first; the discovery list. */
  discovered = $state<DiscoveredEntry[]>([]);
  /** The last discovery error (NATS unreachable), shown beside the list. */
  discoveryError = $state("");

  /** The NATS connection discovery lists streams over, and its manager. */
  #discoveryNc: NatsConnection | null = null;
  #discoveryJsm: JetStreamManager | null = null;
  /** The websocket URL the discovery connection is bound to, to spot retargeting. */
  #discoveryServer = "";
  /** The polling timer, so discovery can be torn down. */
  #discoveryTimer: ReturnType<typeof setInterval> | null = null;

  /** A REST client bound to the current control-service URL. */
  get #client(): ControlClient {
    return new ControlClient(this.config.control);
  }

  /**
   * Attach a read-only viewer to a subject named by hand (or by a shared URL),
   * exactly as the original single-episode viewer did — no control plane
   * involved. Adds a panel and returns it.
   */
  observe(server: string, subject: string): WatchedEpisode {
    const viewer = new EpisodeViewer();
    const episode: WatchedEpisode = {
      key: `obs-${nextKey++}`,
      viewer,
      source: "observe",
      view: null,
      title: subject,
    };
    this.episodes = [...this.episodes, episode];
    void viewer.connect(server, subject);
    return episode;
  }

  /**
   * Begin polling JetStream for the set of episodes on the bus.
   *
   * Discovery is a **read over NATS** (ADR-0001): it lists streams directly
   * through the websocket connection, so it works even with the control service
   * down. The service is consulted only to *overlay* lifecycle status and a Stop
   * handle onto episodes it owns; a service failure leaves discovery intact. Poll
   * once immediately, then on an interval so new episodes — live or replayed,
   * from any source — appear without a manual refresh.
   */
  async startDiscovery(intervalMs = 3000): Promise<void> {
    this.stopDiscovery();
    await this.discover();
    this.#discoveryTimer = setInterval(() => void this.discover(), intervalMs);
  }

  /** Stop polling and drop the discovery connection. */
  stopDiscovery(): void {
    if (this.#discoveryTimer !== null) {
      clearInterval(this.#discoveryTimer);
      this.#discoveryTimer = null;
    }
    this.#closeDiscoveryConnection();
  }

  /** Drop just the discovery connection, leaving any poll timer running. */
  #closeDiscoveryConnection(): void {
    const nc = this.#discoveryNc;
    this.#discoveryNc = null;
    this.#discoveryJsm = null;
    this.#discoveryServer = "";
    if (nc && !nc.isClosed()) void nc.close();
  }

  /**
   * Refresh the discovery list once: read every episode of this app off the bus,
   * then overlay the control service's REST view onto the ones it owns.
   *
   * The JetStream read is the source of truth for *existence*; the REST list is
   * best-effort enrichment. If the service is unreachable the discovered episodes
   * still render (without status or Stop), because the wire already knows them.
   */
  async discover(): Promise<void> {
    const jsm = await this.#discoveryManager();
    if (jsm === null) return; // NATS unreachable; #discoveryManager set the error
    let found: DiscoveredEpisode[];
    try {
      found = await discoverEpisodes(jsm, this.config.app);
    } catch (err) {
      this.discoveryError = err instanceof Error ? err.message : String(err);
      return;
    }
    // Best-effort overlay: index the service's owned episodes by subject identity
    // so each discovered stream picks up its lifecycle view when there is one.
    const owned = new Map<string, EpisodeView>();
    try {
      for (const view of await this.#client.listEpisodes()) {
        owned.set(`${view.app}/${view.episode_id}`, view);
      }
    } catch {
      // Service down or unreachable: discovery still stands on its own.
    }
    this.discovered = found.map((episode) => ({
      ...episode,
      view: owned.get(`${episode.app}/${episode.episodeId}`) ?? null,
    }));
    this.discoveryError = "";
  }

  /**
   * Attach a read-only viewer to a discovered episode's public channel — the
   * one-click Watch. When the service owns the episode the panel carries its REST
   * view (live status, a Stop button); otherwise it is observed read-only, the
   * same single transcript path a live episode uses (ADR-0001).
   */
  watchDiscovered(entry: DiscoveredEntry): WatchedEpisode {
    const viewer = new EpisodeViewer();
    const episode: WatchedEpisode = {
      key: `dsc-${nextKey++}`,
      viewer,
      source: entry.view ? "control" : "observe",
      view: entry.view,
      title: `${entry.app}/${entry.episodeId}`,
    };
    this.episodes = [...this.episodes, episode];
    void viewer.connect(this.config.natsWs, `${entry.subjectRoot}.public`);
    return episode;
  }

  /** Ensure a live discovery connection to the configured websocket NATS. */
  async #discoveryManager(): Promise<JetStreamManager | null> {
    // Reuse the connection unless it dropped or the operator retargeted the URL.
    // Only the connection is dropped here; the poll timer keeps running so the
    // list refreshes against the new server.
    if (this.#discoveryServer !== this.config.natsWs) this.#closeDiscoveryConnection();
    if (this.#discoveryNc && !this.#discoveryNc.isClosed() && this.#discoveryJsm) {
      return this.#discoveryJsm;
    }
    try {
      const nc = await wsconnect({
        servers: this.config.natsWs,
        name: "twentyquestions-discovery",
        reconnect: true,
        maxReconnectAttempts: -1,
        reconnectTimeWait: 1000,
        timeout: 5000,
      });
      this.#discoveryNc = nc;
      this.#discoveryServer = this.config.natsWs;
      this.#discoveryJsm = await jetstreamManager(nc);
      return this.#discoveryJsm;
    } catch (err) {
      this.discoveryError = `cannot reach NATS at ${this.config.natsWs} [${String(err)}]`;
      return null;
    }
  }

  /**
   * Configure and launch a live episode over REST, then watch its transcript.
   *
   * The settable surface (`secret`, `model`, `parquetLog`) is mapped onto the
   * service's `config` blocks keyed as the app advertises them; a blank field is
   * omitted. On success the new episode's viewer is connected to its public
   * subject over the configured websocket NATS.
   */
  async startLive(settings: LaunchSettings): Promise<void> {
    const agents: Record<string, ComponentConfig> = {};
    const model = settings.model.trim();
    if (model) {
      for (const name of LLM_AGENTS) agents[name] = { config: { model } };
    }
    const secret = settings.secret.trim();
    if (secret) {
      agents[HOST] = { config: { ...(agents[HOST]?.config ?? {}), secret } };
    }
    const request: CreateEpisodeRequest = { mode: "live" };
    if (Object.keys(agents).length > 0) request.agents = agents;
    const parquetLog = settings.parquetLog.trim();
    if (parquetLog) request.parquet_log = parquetLog;
    const episodeId = settings.episodeId.trim();
    if (episodeId) request.episode_id = episodeId;
    await this.#launch(request);
  }

  /**
   * Replay a recorded Parquet log through the service's replay mode, then watch
   * it through the very same transcript path as a live episode (ADR-0001).
   */
  async startReplay(parquetPath: string): Promise<void> {
    await this.#launch({ mode: "replay", parquet_path: parquetPath.trim() });
  }

  /** POST a create request, then attach a viewer to the returned episode. */
  async #launch(request: CreateEpisodeRequest): Promise<void> {
    this.busy = true;
    this.error = "";
    try {
      const view = await this.#client.createEpisode(this.config.application, request);
      this.#watch(view);
      void this.discover(); // surface the new episode in the list without waiting for the poll
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.busy = false;
    }
  }

  /** Add a panel for a control-launched episode and connect its viewer. */
  #watch(view: EpisodeView): WatchedEpisode {
    const viewer = new EpisodeViewer();
    const episode: WatchedEpisode = {
      key: `ctl-${nextKey++}`,
      viewer,
      source: "control",
      view,
      title: `${view.app}/${view.episode_id}`,
    };
    this.episodes = [...this.episodes, episode];
    void viewer.connect(this.config.natsWs, publicSubjectOf(view));
    return episode;
  }

  /**
   * Gracefully stop a control-launched episode over REST and latch its now
   * terminal view. The transcript stays on screen — JetStream keeps the stream,
   * so the viewer continues to show the completed conversation.
   */
  async stop(episode: WatchedEpisode): Promise<void> {
    if (episode.view === null) return; // observed-only: nothing to stop over REST
    this.busy = true;
    this.error = "";
    try {
      const view = await this.#client.stopEpisode(episode.view.application, episode.view.id);
      this.#replace(episode.key, view);
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.busy = false;
    }
  }

  /**
   * Gracefully stop a discovered episode the service owns, straight from the
   * discovery list (no panel need be open). Only episodes carrying a controllable
   * view can be stopped; the list re-reads afterwards to latch the new status.
   */
  async stopDiscovered(entry: DiscoveredEntry): Promise<void> {
    if (entry.view === null) return; // not service-owned: no handle to stop
    this.busy = true;
    this.error = "";
    try {
      await this.#client.stopEpisode(entry.view.application, entry.view.id);
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.busy = false;
    }
    await this.discover();
  }

  /**
   * Re-fetch the status of every control-launched episode from the service —
   * the "display" that reflects which episodes are running, ended, or aborted.
   * Observed episodes have no REST handle and are left untouched.
   */
  async refresh(): Promise<void> {
    this.error = "";
    await Promise.all(
      this.episodes
        .filter((episode) => episode.view !== null)
        .map(async (episode) => {
          const view = episode.view!;
          try {
            this.#replace(episode.key, await this.#client.getEpisode(view.application, view.id));
          } catch (err) {
            // A single failed refresh should not blank the others; show the last.
            this.error = err instanceof Error ? err.message : String(err);
          }
        }),
    );
  }

  /**
   * Remove a panel and tear its viewer down. Closing a panel only detaches the
   * read-only subscription; it does not stop the episode (use `stop` for that).
   */
  close(episode: WatchedEpisode): void {
    void episode.viewer.disconnect();
    this.episodes = this.episodes.filter((e) => e.key !== episode.key);
  }

  /** Swap in a fresh view for a watched episode, preserving its viewer and key. */
  #replace(key: string, view: EpisodeView): void {
    this.episodes = this.episodes.map((episode) =>
      episode.key === key ? { ...episode, view, title: `${view.app}/${view.episode_id}` } : episode,
    );
  }
}
