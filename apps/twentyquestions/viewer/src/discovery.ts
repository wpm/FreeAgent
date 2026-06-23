/**
 * Episode discovery over JetStream: the wire is the registry.
 *
 * The control service's in-memory registry (`freeagent.service.registry`) only
 * knows episodes *this* service instance launched, this session — it is blind to
 * episodes from the CLI, from another service, or from before a restart (ADR-0002
 * names this a v1 limitation). But the authoritative set of episodes already
 * exists on the wire: every episode, live **or** replayed, gets one JetStream
 * stream `<app>_episode_<id>` capturing `<app>.episode.<id>.>` (see
 * `freeagent.subjects`). This module reads that set directly through the
 * JetStream management API the browser already holds — no REST, no service — so
 * discovery works even with the control service down (ADR-0001: reads over NATS,
 * writes over REST).
 *
 * Identity is parsed from the stream's **capture subject**
 * (`config.subjects[0]` = `<app>.episode.<id>.>`), split on `.`, **not** by
 * splitting the stream name on `_`: an episode id may itself contain `_`, while
 * the subject delimiter `.` is unambiguous (`subjects.validate_name` forbids `.`
 * in ids but allows `_`).
 */
import type { JetStreamManager } from "@nats-io/jetstream";

/**
 * One episode found on the bus, independent of any control service.
 *
 * `app`/`episodeId` are the subject-level identity and `subjectRoot` is what a
 * viewer subscribes under (`<subjectRoot>.public`). `lastActiveAt`,
 * `messageCount`, and `createdAt` come straight from JetStream stream state —
 * existence and last activity, not a lifecycle verdict (a stream cannot tell
 * ended from aborted; that comes from the REST overlay when the service owns it).
 */
export interface DiscoveredEpisode {
  app: string;
  episodeId: string;
  subjectRoot: string;
  /** ISO timestamp of the last message on the stream (`state.last_ts`). */
  lastActiveAt: string;
  /** Number of messages captured so far (`state.messages`). */
  messageCount: number;
  /** ISO timestamp the stream was created (`created`). */
  createdAt: string;
}

/** The subject-level identity carried by an episode capture subject. */
interface EpisodeIdentity {
  app: string;
  episodeId: string;
  subjectRoot: string;
}

/**
 * Parse `<app>.episode.<id>.>` into its identity, or return null if the subject
 * is not a FreeAgent episode capture subject.
 *
 * Splitting on `.` is deliberate: the episode id is a single dot-free segment
 * (`[A-Za-z0-9_-]+`), so even an id containing `_` parses cleanly here where
 * splitting the underscore-joined stream name would not.
 */
export function parseEpisodeSubject(subject: string): EpisodeIdentity | null {
  const parts = subject.split(".");
  // <app>.episode.<id>.>  → exactly four segments, the middle one literal.
  if (parts.length !== 4 || parts[1] !== "episode" || parts[3] !== ">") return null;
  const [app, , episodeId] = parts;
  if (!app || !episodeId) return null;
  return { app, episodeId, subjectRoot: `${app}.episode.${episodeId}` };
}

/**
 * Every episode of *app* currently present on the bus, newest activity first.
 *
 * Lists all JetStream streams over the existing connection and keeps the ones
 * whose capture subject names an episode of *app*. A stream whose first subject
 * does not parse as an episode subject (any non-FreeAgent stream sharing the
 * server) is simply skipped.
 */
export async function discoverEpisodes(
  jsm: JetStreamManager,
  app: string,
): Promise<DiscoveredEpisode[]> {
  const episodes: DiscoveredEpisode[] = [];
  for await (const info of jsm.streams.list()) {
    const subject = info.config.subjects[0];
    if (subject === undefined) continue;
    const identity = parseEpisodeSubject(subject);
    if (identity === null || identity.app !== app) continue;
    episodes.push({
      app: identity.app,
      episodeId: identity.episodeId,
      subjectRoot: identity.subjectRoot,
      lastActiveAt: info.state.last_ts,
      messageCount: info.state.messages,
      createdAt: info.created,
    });
  }
  // Newest activity first, so the episode you most likely want is at the top.
  episodes.sort((a, b) => b.lastActiveAt.localeCompare(a.lastActiveAt));
  return episodes;
}
