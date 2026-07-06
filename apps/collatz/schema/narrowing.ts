/**
 * Compile-checked proof that the generated `message_type` tag forms a sound discriminated union.
 *
 * This snippet is never run; it exists to be type-checked by `tsc --noEmit` (the `check` script and
 * the CI regenerate-and-diff job). Its job is to fail compilation if the discriminant regresses in
 * either of two ways:
 *
 * 1. The tag stops being a literal `const` — if `Message.__init_subclass__` stops narrowing it to
 *    `Literal[cls.__name__]`, or the schema pipeline drops the `const`, the generated
 *    `Chain["message_type"]` widens to `string` and `AssertExactlyChain` below stops compiling.
 * 2. The tag becomes optional — if `application_schema` stops marking `message_type` required, the
 *    generated field becomes `message_type?`, a tag-omitted object becomes assignable to multiple
 *    union members, and `AssertRequiredTag` below stops compiling.
 *
 * Collatz currently defines a single message type, so the *generated* `CollatzMessages` union has
 * one member and narrowing it is trivial. `sumIfChain` therefore demonstrates the narrowing over a
 * union made non-vacuous with a local decoy member, while the two `Assert*` checks pin the
 * regression-prone properties directly to the generated type.
 *
 * This is the viewer-side half of the schema pipeline's guarantee: generated TS narrows on
 * `msg.message_type === 'Chain'`, checked by compiling this snippet in the viewer build.
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
 * Assert the generated discriminant is exactly the `"Chain"` literal, not `string`.
 *
 * If the tag widened to `string`, `IsExactlyChain` is `false` and assigning it to `true` fails.
 */
type AssertExactlyChain = IsExactlyChain<CollatzMessages["message_type"]>;
const generatedTagIsExactlyChain: AssertExactlyChain = true;
void generatedTagIsExactlyChain;

/**
 * Assert the discriminant is *required*, not optional.
 *
 * `undefined extends T` is `true` exactly when the property is optional; requiring `false` fails the
 * build if `application_schema` stops forcing `message_type` into the definition's `required` list
 * and the field regenerates as `message_type?`.
 */
type TagIsOptional = undefined extends CollatzMessages["message_type"] ? true : false;
const generatedTagIsRequired: TagIsOptional = false;
void generatedTagIsRequired;
