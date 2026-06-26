import { afterEach, describe, expect, it, vi } from "vitest";

import { createEpisode, deleteEpisode, listEpisodes, ServiceError } from "./service";

/** Build a `fetch` stub that resolves to one canned `Response`. */
function stubFetch(response: Response): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(response)),
  );
}

/** A JSON `Response` with the given status and body. */
function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

/** Await a promise expected to reject, returning the `ServiceError` it threw. */
async function catchServiceError(promise: Promise<unknown>): Promise<ServiceError> {
  try {
    await promise;
  } catch (error) {
    expect(error).toBeInstanceOf(ServiceError);
    return error as ServiceError;
  }
  throw new Error("expected the call to reject, but it resolved");
}

describe("requestJson error handling", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns the decoded body on success", async () => {
    stubFetch(jsonResponse(200, [{ id: "a", name: "Alpha" }]));
    const episodes = await listEpisodes("http://svc");
    expect(episodes).toEqual([{ id: "a", name: "Alpha" }]);
  });

  it("throws a ServiceError carrying the status", async () => {
    stubFetch(jsonResponse(404, { detail: "no application registered as 'twenty-questions'" }));
    await expect(createEpisode("http://svc", "twenty-questions", { agents: {} })).rejects.toThrow(
      ServiceError,
    );
  });

  it("appends the server's detail to the error message", async () => {
    stubFetch(jsonResponse(404, { detail: "no application registered as 'twenty-questions'" }));
    const error = await catchServiceError(
      createEpisode("http://svc", "twenty-questions", { agents: {} }),
    );
    expect(error.status).toBe(404);
    expect(error.message).toContain("-> 404");
    expect(error.message).toContain("no application registered as 'twenty-questions'");
  });

  it("tolerates a non-JSON body, falling back to the status-only message", async () => {
    stubFetch(new Response("<html>nope</html>", { status: 502 }));
    const error = await catchServiceError(listEpisodes("http://svc"));
    expect(error.status).toBe(502);
    expect(error.message).toContain("-> 502");
    // No `detail:` suffix when the body cannot be parsed.
    expect(error.message).not.toContain("detail");
  });

  it("tolerates an empty body", async () => {
    stubFetch(new Response("", { status: 500 }));
    const error = await catchServiceError(listEpisodes("http://svc"));
    expect(error.status).toBe(500);
    expect(error.message).toContain("-> 500");
  });

  it("tolerates a JSON body without a detail field", async () => {
    stubFetch(jsonResponse(400, { error: "bad" }));
    const error = await catchServiceError(listEpisodes("http://svc"));
    expect(error.message).toContain("-> 400");
    expect(error.message).not.toContain("detail");
  });

  it("ignores a non-string detail value", async () => {
    stubFetch(jsonResponse(400, { detail: { nested: true } }));
    const error = await catchServiceError(listEpisodes("http://svc"));
    expect(error.message).toContain("-> 400");
    expect(error.message).not.toContain("nested");
  });
});

describe("deleteEpisode", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("resolves on 204", async () => {
    // The `Response` constructor rejects a 204 (a null-body status), so fake the
    // shape `deleteEpisode` reads: `ok` and `status`.
    stubFetch({ ok: false, status: 204 } as Response);
    await expect(deleteEpisode("http://svc", "twenty-questions", "ep1")).resolves.toBeUndefined();
  });

  it("throws a ServiceError on a real failure", async () => {
    stubFetch(jsonResponse(409, { detail: "episode is still live" }));
    const error = await catchServiceError(deleteEpisode("http://svc", "twenty-questions", "ep1"));
    expect(error.status).toBe(409);
    expect(error.message).toContain("episode is still live");
  });
});
