/**
 * Viewer-side message types, built on the generated schema contract.
 *
 * Everything under `./generated/` is produced from the FreeAgent JSON Schema
 * (run `pnpm run generate` here, or `pnpm run schemas` at the repo root) and
 * must not be edited by hand. Importing it here also serves as the contract's
 * compile test: if the generated envelope or payload types drift or fail to
 * regenerate, this file stops type-checking (`pnpm run typecheck`).
 */
import type { Envelope } from "./generated/envelope.schema";
import type { GameOver } from "./generated/game_over.schema";

export type { Envelope, GameOver };

/** An envelope known to carry the Host's game-over payload. */
export type GameOverEnvelope = Envelope & { payload: GameOver };

/** Narrow a wire envelope to the game-over signal by its discriminator. */
export function isGameOver(envelope: Envelope): envelope is GameOverEnvelope {
  const payload = envelope.payload as Partial<GameOver> | null | undefined;
  return (
    typeof payload === "object" &&
    payload !== null &&
    payload.type === "game_over"
  );
}
