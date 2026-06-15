"""``python -m freeagent.cli ...`` -- the same dispatch as the ``free-agent`` script."""

from __future__ import annotations

import sys

from . import main

if __name__ == "__main__":
    main(sys.argv[1:])
