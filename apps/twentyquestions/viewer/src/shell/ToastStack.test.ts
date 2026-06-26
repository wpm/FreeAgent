// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, describe, expect, it } from "vitest";

import ToastStack from "./ToastStack.svelte";
import { toasts } from "./toasts.svelte";

/** Clear the shared store and the rendered DOM between tests. */
function reset(): void {
  for (const toast of [...toasts.toasts]) {
    toasts.dismiss(toast.id);
  }
}

describe("ToastStack", () => {
  afterEach(() => {
    cleanup();
    reset();
  });

  it("renders nothing when the store is empty", () => {
    render(ToastStack);
    expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
  });

  it("renders one toast's full message", async () => {
    toasts.add("POST .../episodes -> 404: no application registered");
    render(ToastStack);
    expect(
      await screen.findByText("POST .../episodes -> 404: no application registered"),
    ).not.toBeNull();
  });

  it("stacks every toast in the store", () => {
    toasts.add("first");
    toasts.add("second");
    render(ToastStack);
    expect(screen.getByText("first")).not.toBeNull();
    expect(screen.getByText("second")).not.toBeNull();
  });

  it("marks an error-level toast as an alert", () => {
    toasts.add("boom", "error");
    render(ToastStack);
    expect(screen.getByRole("alert").textContent).toContain("boom");
  });

  it("dismisses a toast when its close button is clicked", async () => {
    toasts.add("dismiss me");
    render(ToastStack);
    const close = screen.getByRole("button", { name: "Dismiss notification" });
    close.click();
    // The store is the source of truth; clicking removes it.
    expect(toasts.toasts).toHaveLength(0);
  });
});
