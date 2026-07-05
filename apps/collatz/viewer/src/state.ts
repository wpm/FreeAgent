/**
 * Client-side view-state derivation from the raw data-plane feed.
 *
 * The API serves application messages opaquely and never computes on them (ADR-0007); everything
 * the viewer shows about game state is derived here, in the browser, from the ordered
 * `DataPlaneRecord` feed. Payload shapes come from the generated types in
 * `../schema/collatz.d.ts` — the engine's pydantic models are the single source of truth — and
 * messages are narrowed on their `message_type` discriminant, exactly as
 * `../schema/narrowing.ts` pins.
 */

import type { CollatzMessages } from "../../schema/collatz.js";
import type { DataPlaneRecord } from "./api.js";

/** One agent's chain as currently known: the longest chain seen for that agent so far. */
export interface AgentChain {
  agent: string;
  numbers: number[];
  /** Whether the chain has reached 1, the Collatz fixed point — the engine's `is_complete`. */
  complete: boolean;
}

/** Every generated Collatz message tag; `satisfies` pins it to the generated discriminant. */
const MESSAGE_TYPES: ReadonlySet<string> = new Set(
  ["Chain"] satisfies CollatzMessages["message_type"][],
);

/**
 * Read a record's payload as a Collatz message, or `null` if it isn't one.
 *
 * The record's `message_type` envelope tag (read by the API without decoding) gates the cast:
 * only a tag naming a generated Collatz type lets the opaque payload be treated as the generated
 * union, which callers then narrow on `message_type`.
 */
export function asCollatzMessage(record: DataPlaneRecord): CollatzMessages | null {
  if (record.message_type === null || !MESSAGE_TYPES.has(record.message_type)) {
    return null;
  }
  return record.payload as CollatzMessages;
}

/**
 * The agent a chain message belongs to, read off the record's subject.
 *
 * Direction is carried by the subject, not the message (the engine's design): chains travel to
 * agents on `{root}.agents.{name}` and back on `{root}.environment.replies.{name}`. In both the
 * agent's name is the final subject token.
 */
export function agentOf(record: DataPlaneRecord): string {
  const tokens = record.subject.split(".");
  return tokens[tokens.length - 1] ?? "";
}

/**
 * Parse the launch form's comma/space-separated starting numbers.
 *
 * The engine's `CollatzConfig` requires positive integers (the Collatz map's domain); rejecting
 * anything else here surfaces the mistake in the form rather than as a failed worker.
 */
export function parseStarts(text: string): number[] {
  return text
    .split(/[\s,]+/)
    .filter((token) => token.length > 0)
    .map((token) => {
      const value = Number(token);
      if (!Number.isInteger(value) || value < 1) {
        throw new Error(`Starting numbers must be positive integers; got "${token}"`);
      }
      return value;
    });
}

/**
 * Fold the data-plane feed into each agent's current chain, sorted by agent name.
 *
 * Chains only ever grow (the engine appends one Collatz step per exchange), so the longest chain
 * seen for an agent is its current state regardless of the order records arrived in.
 */
export function deriveChains(records: DataPlaneRecord[]): AgentChain[] {
  const longest = new Map<string, number[]>();
  for (const record of records) {
    const message = asCollatzMessage(record);
    if (message === null || message.message_type !== "Chain") {
      continue;
    }
    // `message` is narrowed to Chain here by the discriminant check above.
    const agent = agentOf(record);
    const known = longest.get(agent);
    if (known === undefined || message.numbers.length > known.length) {
      longest.set(agent, message.numbers);
    }
  }
  return [...longest.entries()]
    .map(([agent, numbers]) => ({
      agent,
      numbers,
      complete: numbers.length > 0 && numbers[numbers.length - 1] === 1,
    }))
    .sort((a, b) => a.agent.localeCompare(b.agent));
}
