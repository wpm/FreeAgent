"""End-to-end test: ``free-agent twenty-questions run`` on a fake-LLM episode.

This is the Phase 3 acceptance path, exercised through the real CLI:

* a derived copy of ``apps/twentyquestions/examples/twentyquestions-fake.yml``
  is written to a temporary directory (recorder output redirected to a per-test
  path, episode id left auto-generated) and run via
  ``free-agent twenty-questions run`` as a subprocess from the repository root,
  where the ``fake:apps/twentyquestions/examples/fake/*.yml`` model paths
  resolve;
* the episode must reach ``ended``: exit code 0 and ``state=ended`` in the
  runner's summary line;
* the recorder's Parquet file (written before the runner exits) is opened
  with pyarrow and checked for the expected subjects, senders, strict
  ``stream_seq`` ordering, the lifecycle bracket (exactly one ``start`` and
  one ``shutdown``, both from ``env``), the scripted public conversation in
  order (welcome, two answered questions, the winning guess, the GAME OVER
  announcement), the Host's ``game_over`` signal on the environment's inbox,
  and the three goodbyes landing inside the stopping grace period, each after
  the GAME OVER announcement that triggered it.

Skipped cleanly when NATS is not running. Episode ids are auto-generated and
the output path is per-test, so this test never collides with concurrently
running episodes.
"""

from __future__ import annotations

import itertools
import json
import os
import socket
import subprocess
import sys
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import pyarrow.parquet as pq
import pytest
import yaml

from freeagent import default_nats_url

REPO_ROOT = Path(__file__).resolve().parent.parent
EXAMPLE_CONFIG = (
    REPO_ROOT / "apps" / "twentyquestions" / "examples" / "twentyquestions-fake.yml"
)

_PARSED_NATS = urlparse(default_nats_url())
NATS_HOST = _PARSED_NATS.hostname or "localhost"
NATS_PORT = _PARSED_NATS.port or 4222
RUN_TIMEOUT = 120.0

APP = "twentyquestions"
ROSTER = frozenset({"host", "alice", "bob", "carol"})
ALL_SENDERS = ROSTER | {"env"}

#: (sender, distinctive substring) pairs that must appear on the public
#: channel in this order -- the scripted game in apps/twentyquestions/examples/fake/.
PUBLIC_CONVERSATION: tuple[tuple[str, str], ...] = (
    ("host", "Welcome to Twenty Questions"),
    ("host", "question 1 of 20"),
    ("host", "question 2 of 20"),
    ("host", "Correct! An octopus"),
    ("host", "GAME OVER"),
)


def _nats_running() -> bool:
    try:
        with socket.create_connection((NATS_HOST, NATS_PORT), timeout=0.5):
            return True
    except OSError:
        return False


requires_nats = pytest.mark.skipif(
    not _nats_running(),
    reason=(
        "NATS is not running; start it with: docker compose -f docker/nats/docker-compose.yml up -d"
    ),
)

_SLOW_TEST_VAR = "FREEAGENT_SLOW_TEST"
slow = pytest.mark.skipif(
    not os.environ.get(_SLOW_TEST_VAR),
    reason=f"slow test is opt-in: set {_SLOW_TEST_VAR} to run it",
)


def _derive_config(tmp_path: Path) -> tuple[Path, Path]:
    """Write a copy of the fake example to a temp dir; return (config, parquet path).

    Loading and rewriting the example proves the config is machine-readable. The
    episode id stays absent so the runner generates a fresh one. The parquet path
    is per-test and must not exist (``--parquet-log`` refuses to overwrite).
    """
    config: dict[str, Any] = yaml.safe_load(EXAMPLE_CONFIG.read_text(encoding="utf-8"))
    assert "episode_id" not in config, "the example should leave the episode id auto-generated"
    # Target whichever NATS this test run uses ($FREEAGENT_NATS_URL or localhost:4222),
    # so a per-worktree server on a distinct port is honored by the launched episode.
    config["nats_url"] = default_nats_url()
    output = tmp_path / "episode.parquet"
    derived = tmp_path / "twentyquestions-fake.yml"
    derived.write_text(yaml.safe_dump(config), encoding="utf-8")
    return derived, output


def _run_cli(config_path: Path, parquet_log: Path) -> subprocess.CompletedProcess[str]:
    """Run the real CLI from the repo root (the fake: model paths are cwd-relative)."""
    return subprocess.run(
        [
            sys.executable,
            "-m",
            "freeagent.cli",
            "twenty-questions",
            "run",
            str(config_path),
            "--parquet-log",
            str(parquet_log),
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=RUN_TIMEOUT,
        check=False,
    )


def _payload(row: dict[str, Any]) -> Any:
    """The payload column is the JSON serialization of the envelope payload."""
    return json.loads(row["payload"])


@slow
@requires_nats
def test_fake_twentyquestions_episode_end_to_end(tmp_path: Path) -> None:
    derived, output = _derive_config(tmp_path)

    result = _run_cli(derived, output)
    detail = f"--- stdout ---\n{result.stdout}\n--- stderr ---\n{result.stderr}"
    assert result.returncode == 0, (
        f"free-agent twenty-questions run exited {result.returncode}\n{detail}"
    )
    assert "state=ended" in result.stdout, f"missing state=ended in summary\n{detail}"
    # The runner waits for the recorder before exiting, so the file exists now.
    assert output.exists(), f"recorder output {output} was not written\n{detail}"

    rows: list[dict[str, Any]] = pq.read_table(output).to_pylist()
    assert rows, "the recorded Parquet file has no rows"

    # One episode, every row carrying the same auto-generated id.
    episode_ids = {row["episode_id"] for row in rows}
    assert len(episode_ids) == 1, f"expected one episode id, got {episode_ids}"
    episode_id = next(iter(episode_ids))
    root = f"{APP}.episode.{episode_id}"

    # Total order: rows sorted by stream_seq, strictly increasing; the
    # JetStream server clock never runs backwards.
    seqs = [row["stream_seq"] for row in rows]
    assert all(b > a for a, b in itertools.pairwise(seqs)), (
        f"stream_seq not strictly increasing: {seqs}"
    )
    times = [row["received_at"] for row in rows]
    assert all(b >= a for a, b in itertools.pairwise(times)), "received_at not non-decreasing"

    # Every subject lives under this episode's root; senders are the roster
    # plus the environment, nothing else.
    assert all(row["subject"].startswith(f"{root}.") for row in rows)
    assert {row["sender"] for row in rows} == ALL_SENDERS

    # Lifecycle bracket: exactly one start and one shutdown on the control
    # subject, both broadcast by the environment.
    control = [row for row in rows if row["subject"] == f"{root}.control"]
    assert all(row["sender"] == "env" for row in control)
    starts = [row for row in control if _payload(row).get("type") == "start"]
    shutdowns = [row for row in control if _payload(row).get("type") == "shutdown"]
    assert len(starts) == 1, f"expected exactly one start, got {len(starts)}"
    assert len(shutdowns) == 1, f"expected exactly one shutdown, got {len(shutdowns)}"
    shutdown_seq: int = shutdowns[0]["stream_seq"]

    # The public-channel conversation plays out the scripted game in order.
    public: list[dict[str, Any]] = [row for row in rows if row["subject"] == f"{root}.public"]
    assert public, "no public-channel traffic recorded"
    expected = list(PUBLIC_CONVERSATION)
    for row in public:
        message = _payload(row)
        assert isinstance(message, str), f"public payloads are utterances, got {message!r}"
        if expected and row["sender"] == expected[0][0] and expected[0][1] in message:
            expected.pop(0)
    assert not expected, f"public conversation never reached, in order: {expected}"

    # The Host signals game over on the environment's inbox, and only after
    # all the win-announcement traffic does the environment broadcast
    # shutdown: the inbox signal is information, the control broadcast is the
    # decision.
    inbox = [row for row in rows if row["subject"] == f"{root}.env"]
    game_overs = [row for row in inbox if _payload(row).get("type") == "game_over"]
    assert len(game_overs) == 1, f"expected exactly one game_over signal, got {len(game_overs)}"
    assert game_overs[0]["sender"] == "host"
    announcement_seq: int = next(
        row["stream_seq"]
        for row in public
        if row["sender"] == "host" and "GAME OVER" in _payload(row)
    )
    assert announcement_seq < shutdown_seq, "shutdown must follow the GAME OVER announcement"
    assert game_overs[0]["stream_seq"] < shutdown_seq, "shutdown must follow the game_over signal"

    # All three players say goodbye, and the grace period is what lets those
    # goodbyes be published before the processes exit -- their presence in the
    # log is the grace period working.
    #
    # Ordering note: the goodbyes are scripted reactions to the Host's GAME
    # OVER announcement (see apps/twentyquestions/examples/fake/*.yml), and so is the shutdown
    # broadcast (announcement -> game_over signal -> env broadcasts shutdown).
    # Goodbye and shutdown are therefore *concurrent* reactions in separate
    # processes; nothing orders a goodbye after the shutdown in stream
    # sequence, and a quick player routinely beats the env's extra hops. The
    # invariant that actually holds is causal: every goodbye follows the
    # announcement that triggered it. Asserting goodbye-after-shutdown was a
    # race and flaked.
    goodbyes = [
        row for row in public if "goodbye" in _payload(row).lower() and row["sender"] in ROSTER
    ]
    assert {row["sender"] for row in goodbyes} == {"alice", "bob", "carol"}
    assert len(goodbyes) == 3, f"expected exactly three goodbyes, got {len(goodbyes)}"
    assert all(row["stream_seq"] > announcement_seq for row in goodbyes), (
        "each goodbye must follow the GAME OVER announcement that triggered it"
    )
