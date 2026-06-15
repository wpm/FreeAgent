/// <reference types="@vitest/browser/context" />
import { wsconnect, type NatsConnection } from "@nats-io/nats-core";
import { jetstream, jetstreamManager } from "@nats-io/jetstream";
import { page } from "@vitest/browser/context";
import { render } from "vitest-browser-svelte";
import { afterAll, beforeAll, expect, test } from "vitest";

import App from "./App.svelte";
import { publicSubject } from "./config";

// Closes the "browser verification gap": this test loads the real viewer in a
// headless browser and drives the actual transport (`wsconnect` over a real
// WebSocket → JetStream consumer → Svelte render), against a live NATS. It seeds
// a few public-channel frames exactly as a live episode or replay would put them
// on the wire (a FreeAgent Envelope per message, byte-identical subjects), then
// asserts the chat transcript — including the Host's outcome line — renders.
//
// Requires a NATS server with the websocket listener (docker/nats); the URL is
// VITE_NATS_WS_URL or ws://localhost:8080.

const SERVER = import.meta.env.VITE_NATS_WS_URL ?? "ws://localhost:8080";

// A fresh episode per run so reused JetStream state never bleeds between runs.
const EPISODE = `vitest${Date.now()}`;
const SUBJECT = publicSubject("twentyquestions", EPISODE);
const STREAM = `twentyquestions_episode_${EPISODE}`;

const FRAMES = [
  { sender: "alice", text: "Host, is it an animal?" },
  { sender: "host", text: "Yes, it is an animal." },
  { sender: "host", text: "GAME OVER -- the Players win! The secret was an octopus." },
];

let nc: NatsConnection;

beforeAll(async () => {
  nc = await wsconnect({ servers: SERVER, name: "vitest-seed", timeout: 5000 });
  const jsm = await jetstreamManager(nc);
  // Mirror the engine's per-episode stream: capture every subject under the root.
  await jsm.streams.add({ name: STREAM, subjects: [`twentyquestions.episode.${EPISODE}.>`] });
  const js = jetstream(nc);
  for (const frame of FRAMES) {
    const envelope = {
      message_id: crypto.randomUUID().replace(/-/g, ""),
      episode_id: EPISODE,
      sender: frame.sender,
      payload: frame.text,
    };
    await js.publish(SUBJECT, JSON.stringify(envelope));
  }
});

afterAll(async () => {
  await nc?.drain();
});

test("renders the public transcript live in a real browser", async () => {
  render(App);

  // Drive the connect bar rather than relying on URL params (the test harness
  // owns the page URL): point it at the seeded episode and connect.
  await page.getByPlaceholder("ws://localhost:8080").fill(SERVER);
  await page.getByPlaceholder(/\.public/).fill(SUBJECT);
  await page.getByRole("button", { name: "Connect" }).click();

  // Every seeded frame should appear as a bubble, in order, decoded from the
  // Envelope payload — including the Host's in-world game-over announcement.
  for (const frame of FRAMES) {
    await expect.element(page.getByText(frame.text)).toBeVisible();
  }
  // And the connection should report itself live.
  await expect.element(page.getByText("Live")).toBeVisible();
});
