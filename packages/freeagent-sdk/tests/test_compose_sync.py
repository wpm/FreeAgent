"""Guard that the in-repo and packaged NATS compose files stay in sync.

The in-repo ``docker/compose.yaml`` is the source of truth for this repo; the SDK ships a copy as
package data (:mod:`freeagent.sdk._platform`) so the platform comes up from a third-party repo that
only depends on ``freeagent-sdk``. Two copies drift silently: an edit to one (a new image tag, a
changed port, a healthcheck tweak) that misses the other leaves in-repo and out-of-repo sessions on
different platforms. This test holds the line until the two files can be unified.

The one deliberate difference is the config volume's *source* path: the repo keeps the NATS config
under ``docker/nats/``, while the packaged copy ships it beside the compose file, so the relative
mount differs. That single line is normalized away; everything else must match exactly (see issue
#115, review finding F6).
"""

from __future__ import annotations

from importlib import resources
from pathlib import Path

_REPO_COMPOSE = Path(__file__).resolve().parents[3] / "docker" / "compose.yaml"
"""The in-repo compose file: <repo>/docker/compose.yaml, four levels up from this test."""

_CONFIG_MOUNT_TARGET = ":/etc/nats/nats-server.conf:ro"
"""The container-side half of the NATS config bind mount, identical in both files; only the host-
side source path before it differs, and that difference is the one this test normalizes away."""


def _significant_lines(text: str) -> list[str]:
    """Return a compose file's meaningful lines: comments and blank lines stripped, config mount
    source normalized.

    Comments and blank lines are dropped because they are documentation, not configuration — the
    packaged copy carries an extra explanatory comment on the mount by design. The config bind
    mount's host-side source is collapsed to a single sentinel so the one deliberate path difference
    (``docker/nats/nats-server.conf`` in-repo versus a sibling file in the package) does not trip
    the guard, while every other byte of every other line still must match.

    :param text: The raw contents of a compose file.
    :return: The stripped, normalized significant lines, in order.
    """
    lines: list[str] = []
    for raw in text.splitlines():
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.endswith(_CONFIG_MOUNT_TARGET):
            lines.append(f"- <config-mount>{_CONFIG_MOUNT_TARGET}")
            continue
        lines.append(stripped)
    return lines


def test_packaged_compose_matches_the_in_repo_source_of_truth() -> None:
    # If this fails, an edit landed in one compose file but not the other. Reconcile them (leaving
    # only the config-mount source path different) so in-repo and out-of-repo sessions run the same
    # platform. See issue #115 F6.
    packaged = resources.files("freeagent.sdk._platform").joinpath("compose.yaml").read_text()
    assert _significant_lines(_REPO_COMPOSE.read_text()) == _significant_lines(packaged)
