"""Engine-free launch identity for Twenty Questions (ADR-0005).

This module is the app's :class:`~freeagent.cli.apps.ManifestSpec` advertised on
the ``freeagent.manifests`` entry-point group. It is deliberately a **top-level
module that imports no engine code** -- only class-ref *strings* -- so the slim
worker-pool service can resolve it (and enqueue this app's manifests) **without
importing** ``twentyquestions`` itself. Importing ``twentyquestions.cli`` (the
:class:`~freeagent.AppSpec`) would pull in the environment, host, and player
classes; this module pulls in nothing, which is the whole point of the slim
image's ``create`` path (no ``404 unknown application`` for an app whose engine
is not installed).

The strings here must stay in lockstep with ``twentyquestions.cli``'s
:data:`~twentyquestions.cli.APP`; a parity test in the engine's test suite
asserts that :meth:`AppSpec.manifest_spec` produces exactly this spec, so the two
descriptions cannot silently drift.
"""

from __future__ import annotations

from freeagent.cli.apps import ManifestSpec

#: The undashed subject prefix (matches ``twentyquestions.cli.APP_NAME``).
APP_NAME = "twentyquestions"

#: The launch identity as ``module:QualName`` strings -- no engine import. The
#: worker that runs a manifest imports the named class; the service never does.
MANIFEST = ManifestSpec(
    name=APP_NAME,
    environment="twentyquestions.environment:TwentyQuestionsEnvironment",
    roster={
        "host": "twentyquestions.host:Host",
        "alice": "twentyquestions.player:Player",
        "bob": "twentyquestions.player:Player",
        "carol": "twentyquestions.player:Player",
    },
)
