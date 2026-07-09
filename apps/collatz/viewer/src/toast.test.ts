/**
 * Tests for the pure toast logic in `toast.ts`: how each kind of thrown error is formatted, and
 * the store's coalescing rule (issue #118). The DOM face of toasts is `main.ts` wiring, untested
 * here like the rest of that module.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { ApiError } from "./api.js";
import { formatToast, MAX_BODY_LENGTH, ToastStore } from "./toast.js";

test("an ApiError formats as a method/path/status title with the detail as body", () => {
  const error = new ApiError("GET", "/applications/collatz/episodes", 404, "not installed");
  assert.deepEqual(formatToast(error), {
    title: "GET /applications/collatz/episodes → 404",
    body: "not installed",
  });
});

test("an ApiError with an empty detail still gets a readable body", () => {
  const formatted = formatToast(new ApiError("GET", "/applications", 502, ""));
  assert.equal(formatted.title, "GET /applications → 502");
  assert.notEqual(formatted.body, "");
});

test("a plain Error formats as its message with no title", () => {
  assert.deepEqual(formatToast(new Error("Give at least one starting number")), {
    title: null,
    body: "Give at least one starting number",
  });
});

test("a non-Error throw is stringified", () => {
  assert.deepEqual(formatToast("boom"), { title: null, body: "boom" });
});

test("an oversized body is truncated with an ellipsis", () => {
  const page = "<html>".repeat(200);
  const formatted = formatToast(new ApiError("GET", "/x", 502, page));
  assert.equal(formatted.body.length, MAX_BODY_LENGTH + 1);
  assert.ok(formatted.body.endsWith("…"));
  assert.ok(formatted.body.startsWith("<html>"));
});

test("a body at the limit is untouched", () => {
  const exact = "x".repeat(MAX_BODY_LENGTH);
  assert.equal(formatToast(new Error(exact)).body, exact);
});

test("an identical error coalesces into the live toast with a bumped count", () => {
  const store = new ToastStore();
  const content = { title: "GET /x → 500", body: "worker died" };
  const first = store.add(content);
  assert.equal(first.kind, "created");
  const second = store.add({ ...content });
  assert.deepEqual(second, { kind: "repeated", id: first.id, count: 2 });
  const third = store.add({ ...content });
  assert.deepEqual(third, { kind: "repeated", id: first.id, count: 3 });
});

test("a different error gets its own toast", () => {
  const store = new ToastStore();
  const first = store.add({ title: null, body: "one" });
  const second = store.add({ title: null, body: "two" });
  assert.equal(first.kind, "created");
  assert.equal(second.kind, "created");
  assert.notEqual(first.id, (second as { id: number }).id);
});

test("same body under different titles does not coalesce", () => {
  const store = new ToastStore();
  store.add({ title: "GET /x → 500", body: "oops" });
  const other = store.add({ title: "POST /x → 500", body: "oops" });
  assert.equal(other.kind, "created");
});

test("dismissal resets coalescing: the same error afterwards is a fresh toast", () => {
  const store = new ToastStore();
  const content = { title: null, body: "connection refused" };
  const first = store.add(content);
  assert.equal(first.kind, "created");
  store.dismiss(first.id);
  const again = store.add(content);
  assert.equal(again.kind, "created");
  assert.notEqual((again as { id: number }).id, first.id);
});
