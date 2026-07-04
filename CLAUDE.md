# CLAUDE.md

Guidance for AI coding agents (Claude Code web and local) working in this
repository.

## Coding Conventions
- Do test-driven developement. Never check in a failing test.
- Try to keep test converage near 100%.
- When writing in Python, include type hints for everything.

## Commit and PR conventions

- The `freeagent` package and applications under `apps` may all be released independently.
- **PR titles MUST follow [Conventional Commits](https://www.conventionalcommits.org/):** 
- **PR titles MUST follow [Conventional Commits](https://www.conventionalcommits.org/):**
- Types and their release effect on `freeagent`: `fix` → patch bump, `feat` →
  minor bump, `!` suffix or `BREAKING CHANGE` footer → breaking bump
  (compressed semver below 1.0). Also available: `docs`, `refactor`, `perf`,
  `test`, `build`, `ci`, `chore`, `revert`.
- Branch commits are squashed away, so their messages are not enforced —
  but use the conventional format there too; it keeps history legible and
  PR titles honest.
- After creating a PR, monitor it and fix all problems: source conflicts, branch updates, and CI failures.
- After a PR clears all checks, run `/review` and address all the comments.
- When creating a PR for an issue, ensure that it when it closes, the issue
  closes as well.
- Keep the history linear: no merges.
