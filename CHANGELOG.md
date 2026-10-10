# Changelog

Notable changes to Free Agent, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The release
workflow takes a version's notes from here.

## [Unreleased]

### Added

- The `free-agent` library: episodes of actors that talk to each other by
  statement, request, and reply on the Tokio runtime.
- The `social-deduction` application, which plays Werewolf with the roles
  dealt at random.

### Changed

- The command line is `social-deduction werewolf uniform-random`, a game
  and then its variant, each a subcommand with its own `--help`. The role
  counts `--werewolves`, `--villagers`, `--doctors` and `--seers` belong to
  the variant; the 0.1.0 top-level flags are gone.

[Unreleased]: https://github.com/wpm/FreeAgent/compare/main...HEAD
