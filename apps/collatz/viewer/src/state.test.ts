/**
 * Tests for the client-side state derivation in `state.ts`, run with Node's built-in test
 * runner against the compiled output (`npm test`). Pure logic only — the DOM wiring in
 * `main.ts` is exercised end to end against a live API, not here.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import type { DataPlaneRecord } from "./api.js";
import { agentOf, asCollatzMessage, deriveChains, parseStarts } from "./state.js";

const ROOT = "episode.collatz.ep1";

function chainRecord(subject: string, numbers: number[], seq = 0): DataPlaneRecord {
  return {
    seq,
    subject,
    message_type: "Chain",
    received_at: 1.0,
    payload: { message_type: "Chain", numbers },
  };
}

test("asCollatzMessage reads a Chain payload through the envelope tag", () => {
  const message = asCollatzMessage(chainRecord(`${ROOT}.agents.agent-0`, [8, 4]));
  assert.notEqual(message, null);
  assert.equal(message?.message_type, "Chain");
  assert.deepEqual(message?.numbers, [8, 4]);
});

test("asCollatzMessage rejects unknown and missing envelope tags", () => {
  const unknown: DataPlaneRecord = {
    seq: 0,
    subject: `${ROOT}.x`,
    message_type: "NoSuchType",
    received_at: 1.0,
    payload: { message_type: "NoSuchType" },
  };
  const untagged: DataPlaneRecord = { ...unknown, message_type: null, payload: [1, 2, 3] };
  assert.equal(asCollatzMessage(unknown), null);
  assert.equal(asCollatzMessage(untagged), null);
});

test("agentOf reads the agent off both chain-bearing subject shapes", () => {
  assert.equal(agentOf(chainRecord(`${ROOT}.agents.agent-0`, [8])), "agent-0");
  assert.equal(agentOf(chainRecord(`${ROOT}.environment.replies.agent-1`, [8])), "agent-1");
});

test("deriveChains keeps each agent's longest chain, sorted by agent", () => {
  const chains = deriveChains([
    chainRecord(`${ROOT}.agents.agent-1`, [3], 0),
    chainRecord(`${ROOT}.agents.agent-0`, [8], 1),
    chainRecord(`${ROOT}.environment.replies.agent-0`, [8, 4], 2),
    chainRecord(`${ROOT}.environment.replies.agent-1`, [3, 10], 3),
    chainRecord(`${ROOT}.agents.agent-1`, [3, 10], 4),
  ]);
  assert.deepEqual(chains, [
    { agent: "agent-0", numbers: [8, 4], complete: false },
    { agent: "agent-1", numbers: [3, 10], complete: false },
  ]);
});

test("deriveChains never shrinks a chain on out-of-order records", () => {
  const chains = deriveChains([
    chainRecord(`${ROOT}.agents.agent-0`, [8, 4, 2], 0),
    chainRecord(`${ROOT}.agents.agent-0`, [8, 4], 1),
  ]);
  assert.deepEqual(chains, [{ agent: "agent-0", numbers: [8, 4, 2], complete: false }]);
});

test("deriveChains marks a chain complete once it reaches 1", () => {
  const chains = deriveChains([chainRecord(`${ROOT}.environment.replies.agent-0`, [8, 4, 2, 1])]);
  assert.deepEqual(chains, [{ agent: "agent-0", numbers: [8, 4, 2, 1], complete: true }]);
});

test("parseStarts accepts comma- and space-separated positive integers", () => {
  assert.deepEqual(parseStarts("27, 9 6"), [27, 9, 6]);
  assert.deepEqual(parseStarts("  "), []);
});

test("parseStarts rejects anything outside the Collatz domain", () => {
  assert.throws(() => parseStarts("0"), /positive integers/);
  assert.throws(() => parseStarts("-3"), /positive integers/);
  assert.throws(() => parseStarts("2.5"), /positive integers/);
  assert.throws(() => parseStarts("seven"), /positive integers/);
});

test("deriveChains ignores records that are not Collatz messages", () => {
  const noise: DataPlaneRecord = {
    seq: 0,
    subject: `${ROOT}.x`,
    message_type: "NoSuchType",
    received_at: 1.0,
    payload: { message_type: "NoSuchType", numbers: [9, 9, 9] },
  };
  assert.deepEqual(deriveChains([noise]), []);
});
