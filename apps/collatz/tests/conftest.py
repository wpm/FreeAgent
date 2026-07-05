"""Test configuration for the Collatz app's unit tests.

Under pytest's ``importlib`` import mode (set repo-wide so same-named test modules across workspace
members don't collide), pytest no longer prepends each test file's directory to ``sys.path``. This
puts *this* directory back on the path so the tests' bare sibling-helper import ``from fakes import
...`` resolves.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
