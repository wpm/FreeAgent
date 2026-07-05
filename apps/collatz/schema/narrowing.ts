/**
 * Compile-checked proof that the generated `message_type` tag narrows a discriminated union.
 *
 * This snippet is never run; it exists to be type-checked by `tsc --noEmit` (the `check` script and
 * the CI regenerate-and-diff job). Its job is to fail compilation if the `message_type` tag ever
 * stops being emitted as a `const` — i.e. if the engine's `Message.__init_subclass__` stops
 * narrowing the tag to `Literal[cls.__name__]`, or the schema pipeline drops the `const`. In that
 * case the generated `Chain["message_type"]` widens from the `"Chain"` literal to `string`, and the
 * assertions below stop compiling.
 *
 * Two things are checked, because a single-member union narrows trivially:
 *
 * 1. `sumIfChain` demonstrates the acceptance criterion literally — `msg.message_type === "Chain"`
 *    selecting `Chain` out of a union — over a union made non-vacuous with a local decoy member.
 * 2. `AssertExactlyChain` pins the generated tag *type* to exactly the `"Chain"` literal, so a
 *    widening to `string` (which the narrowing in (1) alone would tolerate, since the tag is
 *    optional) is still caught.
 *
 * Together they are the viewer-side half of ADR-0007's acceptance criterion: "Generated TS narrows
 * on msg.message_type === 'Chain' (checked by compiling a snippet in the viewer build)."
 */
import type { Chain, CollatzMessages } from "./collatz.d.ts";

/**
 * A message the viewer might also see but that is not a `Chain`, used only to give the union below a
 * second member so the discriminant has something to narrow away. Its `message_type` is a
 * *different* literal, exactly the shape a second generated message type would take.
 */
interface OtherMessage {
  message_type: "Other";
  detail: string;
}

/** A genuine multi-member union, so narrowing on the tag is not vacuous. */
type ObservedMessage = Chain | OtherMessage;

/**
 * Sum a chain's numbers, or return null for any other observed message.
 *
 * Only inside the `message_type === "Chain"` guard may `msg` be used as a `Chain`.
 */
export function sumIfChain(msg: ObservedMessage): number | null {
  if (msg.message_type === "Chain") {
    const narrowed: Chain = msg;
    return narrowed.numbers.reduce((total, n) => total + n, 0);
  }
  return null;
}

/**
 * Resolve to `true` only when `T` is exactly the `"Chain"` string literal, else `false`.
 *
 * A mutual-assignability check: `T extends "Chain"` rejects anything wider (`string`), and
 * `"Chain" extends T` rejects anything narrower or unrelated (`never`, `"Other"`).
 */
type IsExactlyChain<T> = [T] extends ["Chain"]
  ? ["Chain"] extends [T]
    ? true
    : false
  : false;

/**
 * Compile-time assertion that the generated discriminant is exactly `"Chain"`.
 *
 * `NonNullable` drops the `undefined` the optional tag contributes; what remains must be the bare
 * `"Chain"` literal. If the tag widened to `string`, `IsExactlyChain` is `false`, and assigning it
 * to `true` is a type error that fails the build.
 */
type AssertExactlyChain = IsExactlyChain<NonNullable<CollatzMessages["message_type"]>>;
const generatedTagIsExactlyChain: AssertExactlyChain = true;
void generatedTagIsExactlyChain;
