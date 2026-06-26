import { describe, expect, it } from "vitest";

import { ToastStore } from "./toasts.svelte";

describe("ToastStore", () => {
  it("starts empty", () => {
    const store = new ToastStore();
    expect(store.toasts).toEqual([]);
  });

  it("adds a toast and returns its id", () => {
    const store = new ToastStore();
    const id = store.add("something broke");
    expect(store.toasts).toHaveLength(1);
    expect(store.toasts[0]).toMatchObject({ id, message: "something broke", level: "error" });
  });

  it("defaults the level to error but accepts an override", () => {
    const store = new ToastStore();
    store.add("heads up", "info");
    expect(store.toasts[0].level).toBe("info");
  });

  it("gives every toast a distinct id", () => {
    const store = new ToastStore();
    const a = store.add("first");
    const b = store.add("second");
    expect(a).not.toBe(b);
    expect(store.toasts.map((t) => t.id)).toEqual([a, b]);
  });

  it("stacks multiple toasts in insertion order", () => {
    const store = new ToastStore();
    store.add("first");
    store.add("second");
    expect(store.toasts.map((t) => t.message)).toEqual(["first", "second"]);
  });

  it("dismisses a toast by id, leaving the rest", () => {
    const store = new ToastStore();
    const a = store.add("first");
    const b = store.add("second");
    store.dismiss(a);
    expect(store.toasts.map((t) => t.id)).toEqual([b]);
  });

  it("ignores a dismiss for an unknown id", () => {
    const store = new ToastStore();
    const a = store.add("first");
    store.dismiss(a + 999);
    expect(store.toasts.map((t) => t.id)).toEqual([a]);
  });

  it("does not auto-dismiss; a toast persists until dismissed", () => {
    const store = new ToastStore();
    store.add("persistent error");
    // No timers, no auto-removal: the toast is still present.
    expect(store.toasts).toHaveLength(1);
  });
});
