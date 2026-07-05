# Collatz

A deterministic, LLM-free Free Agent application. See the package docstring in
`src/freeagent/app/collatz/__init__.py` for what it does and why it exists.

The Python engine lives under `src/`; its message vocabulary is exported as generated JSON
Schema and TypeScript in [`schema/`](schema/README.md), which the browser viewer in
[`viewer/`](viewer/README.md) builds against. Engine and viewer are co-versioned: they live and
ship together, and CI regenerates the schema artifacts and builds the viewer, so a green build
means all three agree.
