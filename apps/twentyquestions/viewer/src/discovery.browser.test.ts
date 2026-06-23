/// <reference types="@vitest/browser/context" />
import { wsconnect, type NatsConnection } from "@nats-io/nats-core";
import { jetstream, jetstreamManager } from "@nats-io/jetstream";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { afterAll, beforeAll, expect, test } from "vitest";

import App from "./App.svelte";
import { discoverEpisodes, parseEpisodeSubject } from "./discovery";

// Exercises the JetStream discovery path against a real NATS: seed per-episode
// streams exactly as a live episode or replay would (one stream
// `<app>_episode_<id>` capturing `<app>.episode.<id>.>`), then assert
// `discoverEpisodes` finds them off the wire with no control service in sight —
// the whole point of "JetStream is the source of truth". Crucially, none of
// these episodes is in any in-memory registry; discovery is registry-independent
// by construction, because it only ever reads streams.
//
// Requires a NATS server with the websocket listener (docker/nats); the URL is
// VITE_NATS_WS_URL or ws://localhost:8080.

const SERVER = import.meta.env.VITE_NATS_WS_URL ?? "ws://localhost:8080";
const APP = "twentyquestions";

// A unique suffix per run so reused JetStream state never bleeds between runs.
const RUN = `${Date.now()}`;
// One plain id and one whose id itself contains "_": the case that breaks any
// scheme that recovers identity by splitting the underscore-joined stream name.
const PLAIN = `disco${RUN}`;
const UNDERSCORED = `disco_run_${RUN}`;
const SEEDED = [PLAIN, UNDERSCORED];

let nc: NatsConnection;

beforeAll(async () => {
  nc = await wsconnect({ servers: SERVER, name: "vitest-discovery-seed", timeout: 5000 });
  const jsm = await jetstreamManager(nc);
  const js = jetstream(nc);
  for (const episode of SEEDED) {
    await jsm.streams.add({
      name: `${APP}_episode_${episode}`,
      subjects: [`${APP}.episode.${episode}.>`],
    });
    // A couple of frames so the stream reports a non-zero message count.
    const subject = `${APP}.episode.${episode}.public`;
    await js.publish(subject, JSON.stringify({ sender: "alice", payload: "hi" }));
    await js.publish(subject, JSON.stringify({ sender: "host", payload: "hello" }));
  }
});

afterAll(async () => {
  // Leave the streams in place is harmless, but tidy up this run's so the volume
  // does not grow unboundedly across runs.
  try {
    const jsm = await jetstreamManager(nc);
    for (const episode of SEEDED) await jsm.streams.delete(`${APP}_episode_${episode}`);
  } catch {
    // Best-effort cleanup; a failure here must not fail the suite.
  }
  await nc?.drain();
});

test("parseEpisodeSubject recovers identity, including ids that contain '_'", () => {
  expect(parseEpisodeSubject("twentyquestions.episode.abc.>")).toEqual({
    app: "twentyquestions",
    episodeId: "abc",
    subjectRoot: "twentyquestions.episode.abc",
  });
  // An id with underscores survives because we split on the unambiguous ".".
  expect(parseEpisodeSubject("twentyquestions.episode.a_b_c.>")).toEqual({
    app: "twentyquestions",
    episodeId: "a_b_c",
    subjectRoot: "twentyquestions.episode.a_b_c",
  });
  // Not an episode capture subject → not discoverable.
  expect(parseEpisodeSubject("twentyquestions.episode.abc.public")).toBeNull();
  expect(parseEpisodeSubject("KV_some_bucket.>")).toBeNull();
  expect(parseEpisodeSubject("other.thing.x.>")).toBeNull();
});

test("discoverEpisodes lists this app's episodes straight off the bus", async () => {
  const jsm = await jetstreamManager(nc);
  const found = await discoverEpisodes(jsm, APP);
  const byId = new Map(found.map((e) => [e.episodeId, e]));

  for (const episode of SEEDED) {
    const entry = byId.get(episode);
    expect(entry, `expected to discover ${episode}`).toBeDefined();
    expect(entry!.app).toBe(APP);
    expect(entry!.subjectRoot).toBe(`${APP}.episode.${episode}`);
    expect(entry!.messageCount).toBe(2);
    expect(Date.parse(entry!.lastActiveAt)).not.toBeNaN();
    expect(Date.parse(entry!.createdAt)).not.toBeNaN();
  }
});

test("discoverEpisodes filters out other applications' streams", async () => {
  const other = `otherapp_episode_${RUN}`;
  const jsm = await jetstreamManager(nc);
  await jsm.streams.add({ name: other, subjects: [`otherapp.episode.${RUN}.>`] });
  try {
    const found = await discoverEpisodes(jsm, APP);
    expect(found.some((e) => e.app !== APP)).toBe(false);
    expect(found.some((e) => e.episodeId === PLAIN)).toBe(true);
  } finally {
    await jsm.streams.delete(other);
  }
});

test("the controller surfaces a discovered episode and watches it in one click", async () => {
  // No control service is running here, so the REST overlay simply fails and is
  // swallowed — yet the seeded episode (which no in-memory registry knows about)
  // still appears and is watchable purely from JetStream. That is the headline.
  render(App);
  await page.getByPlaceholder("ws://localhost:8080").fill(SERVER);

  // The discovery poll picks up the seeded stream and renders its id with a Watch.
  const item = page.getByRole("listitem").filter({ hasText: PLAIN });
  await expect.element(item).toBeVisible();
  await item.getByRole("button", { name: "Watch" }).click();

  // Watch attaches the read-only viewer to the public channel through the very
  // same transcript path a live episode uses; the seeded frames render.
  await expect.element(page.getByText("hello")).toBeVisible();
});
