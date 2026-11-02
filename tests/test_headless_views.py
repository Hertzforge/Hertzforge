"""The four scriptable faces `test_headless.py` names as strings and
nothing called: each view the app draws has a plain-Python call that
draws the same thing to a file, and a name in an inventory is not a
promise kept. Each is run here, off screen, and has to leave a
picture behind."""

from __future__ import annotations


def _picture(path):
    assert path.exists() and path.stat().st_size > 1000, path
