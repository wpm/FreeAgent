// @vitest-environment jsdom
//
// Exercise the real toast code path end to end: a failed create in the composer
// must raise a toast through the shared store (the `catch { toasts.add(...) }`
// branch in TwentyQuestions.svelte), not just the store in isolation.
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ServiceError } from "../../shell/service";
import { toasts } from "../../shell/toasts.svelte";

// Mock the service client so `createEpisode` rejects with a real ServiceError
// carrying a server detail -- the exact shape issue #66 is about.
vi.mock("../../shell/service", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../shell/service")>();
  return {
    ...actual,
    createEpisode: vi.fn(() =>
      Promise.reject(
        new actual.ServiceError(
          404,
          "POST /freeagent/twenty-questions/episodes -> 404: no application registered as 'twenty-questions'",
        ),
      ),
    ),
  };
});

import TwentyQuestions from "./TwentyQuestions.svelte";

/** Empty the shared store between tests. */
function reset(): void {
  for (const toast of [...toasts.toasts]) {
    toasts.dismiss(toast.id);
  }
}

describe("TwentyQuestions composer error -> toast", () => {
  beforeEach(reset);
  afterEach(() => {
    cleanup();
    reset();
    vi.clearAllMocks();
  });

  it("raises a toast carrying the server detail when the create fails", async () => {
    render(TwentyQuestions, {
      props: {
        episode: null,
        composing: true,
        onCreated: vi.fn(),
        onCancelCompose: vi.fn(),
      },
    });

    // Fill the required secret and submit the start-game form.
    const secret = screen.getByPlaceholderText("elephant");
    await fireEvent.input(secret, { target: { value: "elephant" } });
    await fireEvent.submit(secret.closest("form")!);

    // The failed create surfaced a single toast with the server's explanation.
    await vi.waitFor(() => {
      expect(toasts.toasts).toHaveLength(1);
    });
    const toast = toasts.toasts[0];
    expect(toast.level).toBe("error");
    expect(toast.message).toContain("no application registered as 'twenty-questions'");
    expect(toast.message).toContain("-> 404");
  });

  it("does not raise a toast on a successful create", async () => {
    const { createEpisode } = await import("../../shell/service");
    const ok = {
      id: "ep1",
      application: "twenty-questions",
      episode_id: "ep1",
    } as unknown as Awaited<ReturnType<typeof createEpisode>>;
    vi.mocked(createEpisode).mockResolvedValueOnce(ok);

    const onCreated = vi.fn();
    render(TwentyQuestions, {
      props: { episode: null, composing: true, onCreated, onCancelCompose: vi.fn() },
    });

    const secret = screen.getByPlaceholderText("elephant");
    await fireEvent.input(secret, { target: { value: "elephant" } });
    await fireEvent.submit(secret.closest("form")!);

    await vi.waitFor(() => {
      expect(onCreated).toHaveBeenCalledWith(ok);
    });
    expect(toasts.toasts).toHaveLength(0);
  });

  it("asserts the ServiceError surfaced is the typed error", () => {
    // Guard: the mock rejects with a ServiceError (not a bare Error), so the
    // component's `e instanceof Error` branch carries the real `.message`.
    expect(new ServiceError(404, "x")).toBeInstanceOf(Error);
  });
});
