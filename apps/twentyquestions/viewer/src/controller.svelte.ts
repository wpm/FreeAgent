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
import { resolveControllerConfig, type ControllerConfig } from "./config";
import {
  ControlClient,
  publicSubjectOf,
  type ComponentConfig,
  type CreateEpisodeRequest,
  type EpisodeView,
} from "./control";
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
