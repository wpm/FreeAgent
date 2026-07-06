# Collatz message schema

Generated artifacts that give a viewer the Collatz engine's message shapes without
hand-maintaining them. Python is the single source of truth: the engine's pydantic
models generate a JSON Schema document, which generates TypeScript.

## Files

| File | Checked in? | What it is |
| --- | --- | --- |
| `collatz.schema.json` | yes (generated) | JSON Schema for every Collatz message type, one `$defs` entry each, `message_type` emitted as a `const`. Produced by `freeagent schema collatz`. |
| `collatz.d.ts` | yes (generated) | TypeScript types produced from the schema by `json-schema-to-typescript`. |
| `narrowing.ts` | yes (source) | A `tsc`-checked snippet proving the generated types narrow a discriminated union on `msg.message_type === "Chain"`. Never run; exists to fail compilation if the `const` discriminant regresses. |
| `package.json`, `package-lock.json`, `tsconfig.json` | yes (source) | The Node/TypeScript toolchain for regeneration and the compile check. |

## Regenerating

Both generated files are checked in, and CI regenerates them and fails on any diff — a green build
means the checked-in artifacts match the engine's models. Regenerate them yourself whenever a
Collatz message type changes:

```sh
# 1. schema from the Python engine (run from the repo root, in the uv environment)
uv run freeagent schema collatz > apps/collatz/schema/collatz.schema.json

# 2. TypeScript from the schema (run from this directory)
cd apps/collatz/schema
npm install        # first time only
npm run generate   # collatz.schema.json -> collatz.d.ts
npm run check      # tsc: compiles narrowing.ts against the regenerated types
```

Do not edit `collatz.schema.json` or `collatz.d.ts` by hand; your changes will be overwritten and
CI will reject the drift.
