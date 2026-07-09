/**
 * Tests for the REST client in `api.ts`, run against a stubbed global `fetch` so every request
 * path, unwrapping, and error format is pinned without a server. The real HTTP behavior is
 * exercised end to end against a live API.
 */

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { ApiClient, ApiError } from "./api.js";

const realFetch = globalThis.fetch;

interface Recorded {
  url: string;
  method: string;
  body: string | null;
}

/** Route every fetch to a canned response, recording what was requested. */
function stubFetch(status: number, body: string | null): Recorded[] {
  const calls: Recorded[] = [];
  globalThis.fetch = async (input, init) => {
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? init.body : null,
    });
    return new Response(body, { status });
  };
  return calls;
}

afterEach(() => {
  globalThis.fetch = realFetch;
});

const BASE = "http://api.test";

const STATUS_JSON = JSON.stringify({
  application: "collatz",
  episode_id: "ep1",
  episode_root: "episode.collatz.ep1",
  state: "created",
  agents_alive: [],
  message_count: 0,
  worker_exit_code: null,
});

test("applications unwraps the applications list", async () => {
  const calls = stubFetch(200, JSON.stringify({ applications: ["collatz"] }));
  assert.deepEqual(await new ApiClient(BASE).applications(), ["collatz"]);
  assert.deepEqual(calls, [{ url: `${BASE}/applications`, method: "GET", body: null }]);
});

test("episodes unwraps the episode statuses", async () => {
  const calls = stubFetch(200, JSON.stringify({ episodes: [JSON.parse(STATUS_JSON)] }));
  const episodes = await new ApiClient(BASE).episodes("collatz");
  assert.equal(episodes[0]?.episode_id, "ep1");
  assert.equal(calls[0]?.url, `${BASE}/applications/collatz/episodes`);
});

test("createEpisode posts the episode ID and opaque config", async () => {
  const calls = stubFetch(201, STATUS_JSON);
  const created = await new ApiClient(BASE).createEpisode("collatz", "ep1", { starts: [27] });
  assert.equal(created.state, "created");
  assert.equal(calls[0]?.method, "POST");
  assert.deepEqual(JSON.parse(calls[0]?.body ?? ""), {
    episode_id: "ep1",
    config: { starts: [27] },
  });
});

test("status and messages hit the per-episode routes", async () => {
  const statusCalls = stubFetch(200, STATUS_JSON);
  await new ApiClient(BASE).status("collatz", "ep1");
  assert.equal(statusCalls[0]?.url, `${BASE}/applications/collatz/episodes/ep1`);

  const messageCalls = stubFetch(200, JSON.stringify({ messages: [] }));
  assert.deepEqual(await new ApiClient(BASE).messages("collatz", "ep1"), []);
  assert.equal(messageCalls[0]?.url, `${BASE}/applications/collatz/episodes/ep1/messages`);
});

test("stop tolerates the API's bodyless 204", async () => {
  const calls = stubFetch(204, null);
  await new ApiClient(BASE).stop("collatz", "ep1");
  assert.deepEqual(calls, [
    { url: `${BASE}/applications/collatz/episodes/ep1`, method: "DELETE", body: null },
  ]);
});

test("a failed request reports the method, path, status, and server detail", async () => {
  stubFetch(409, JSON.stringify({ detail: "already exists" }));
  await assert.rejects(
    () => new ApiClient(BASE).createEpisode("collatz", "ep1", {}),
    /POST \/applications\/collatz\/episodes failed \(409\).*already exists/,
  );
});

test("a failed request throws an ApiError carrying the structured fields", async () => {
  stubFetch(404, JSON.stringify({ detail: 'No application named "collatz" is installed' }));
  try {
    await new ApiClient(BASE).episodes("collatz");
    assert.fail("expected a rejection");
  } catch (error) {
    assert.ok(error instanceof ApiError);
    assert.equal(error.method, "GET");
    assert.equal(error.path, "/applications/collatz/episodes");
    assert.equal(error.status, 404);
    // The FastAPI-shaped JSON body's detail is extracted, never pasted raw into prose.
    assert.equal(error.detail, 'No application named "collatz" is installed');
  }
});

test("a non-JSON error body becomes the detail verbatim", async () => {
  stubFetch(502, "Bad Gateway");
  try {
    await new ApiClient(BASE).applications();
    assert.fail("expected a rejection");
  } catch (error) {
    assert.ok(error instanceof ApiError);
    assert.equal(error.detail, "Bad Gateway");
  }
});

test("an unreachable server's bare fetch rejection gains the request's context", async () => {
  globalThis.fetch = async () => {
    throw new TypeError("Failed to fetch");
  };
  await assert.rejects(
    () => new ApiClient(BASE).episodes("collatz"),
    /GET \/applications\/collatz\/episodes: http:\/\/api\.test is unreachable \(Failed to fetch\)/,
  );
});

test("a 422 validation array renders as field: message lines, never raw JSON", async () => {
  stubFetch(
    422,
    JSON.stringify({
      detail: [
        {
          type: "string_pattern_mismatch",
          loc: ["body", "episode_id"],
          msg: "String should match pattern '^[A-Za-z0-9_-]+$'",
          input: "one two",
        },
        { type: "missing", loc: ["body", "config"], msg: "Field required" },
      ],
    }),
  );
  try {
    await new ApiClient(BASE).createEpisode("collatz", "one two", {});
    assert.fail("expected a rejection");
  } catch (error) {
    assert.ok(error instanceof ApiError);
    assert.equal(
      error.detail,
      "episode_id: String should match pattern '^[A-Za-z0-9_-]+$'\nconfig: Field required",
    );
  }
});

test("a validation item whose loc is only plumbing keeps its bare message", async () => {
  stubFetch(422, JSON.stringify({ detail: [{ loc: ["body"], msg: "Field required" }] }));
  try {
    await new ApiClient(BASE).createEpisode("collatz", "ep1", {});
    assert.fail("expected a rejection");
  } catch (error) {
    assert.ok(error instanceof ApiError);
    assert.equal(error.detail, "Field required");
  }
});

test("a detail array outside the validation shape falls back to the raw text", async () => {
  const body = JSON.stringify({ detail: [{ code: 7 }, "mystery"] });
  stubFetch(422, body);
  try {
    await new ApiClient(BASE).createEpisode("collatz", "ep1", {});
    assert.fail("expected a rejection");
  } catch (error) {
    assert.ok(error instanceof ApiError);
    assert.equal(error.detail, body);
  }
});
