"""The app's engine-free :class:`ManifestSpec` must mirror its :class:`AppSpec`.

ADR-0005's slim worker-pool service resolves an app's launch identity through the
``freeagent.manifests`` entry point -- a :class:`~freeagent.cli.apps.ManifestSpec`
of class-ref *strings* that imports no engine code. Twenty Questions advertises
that spec in the top-level ``twentyquestions_manifest`` module, hand-written as
strings. This test is the guard that keeps those strings honest: the strings must
be exactly what :meth:`AppSpec.manifest_spec` derives from the live classes, so a
roster change or a class move in the engine cannot silently break the slim
service's view of the app.
"""

from __future__ import annotations

import twentyquestions_manifest

from twentyquestions.cli import APP


def test_manifest_spec_mirrors_the_appspec() -> None:
    assert APP.manifest_spec() == twentyquestions_manifest.MANIFEST


def test_manifest_spec_carries_the_subject_prefix_name() -> None:
    assert twentyquestions_manifest.MANIFEST.name == APP.name


def test_manifest_module_imports_no_engine_class() -> None:
    """The manifest module is string-only: resolving it imports no engine code.

    Its ``MANIFEST`` references the engine purely by ``module:QualName`` strings;
    nothing in the module is an engine class object (which would defeat the
    slim-service no-import guarantee).
    """
    spec = twentyquestions_manifest.MANIFEST
    assert isinstance(spec.environment, str)
    assert all(isinstance(ref, str) for ref in spec.roster.values())
