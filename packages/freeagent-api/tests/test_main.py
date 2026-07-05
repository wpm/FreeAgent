"""Unit test for the ``freeagent-api`` entry point in :mod:`freeagent.api.__main__`."""

from __future__ import annotations

from typing import Any
from unittest.mock import patch

from freeagent.api.__main__ import main


def test_main_serves_the_module_level_app_with_uvicorn() -> None:
    with patch("freeagent.api.__main__.uvicorn.run") as run:
        main()
    (target,), options = run.call_args
    assert target == "freeagent.api.app:app"
    assert isinstance(options, dict)
    kwargs: dict[str, Any] = options
    assert kwargs["host"] == "127.0.0.1"
