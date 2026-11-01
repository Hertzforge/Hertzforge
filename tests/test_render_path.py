"""Rendering is synchronous, on the thread that asked for it.

On macOS pyvistaqt threads ``render()`` and signals back to the GUI
thread, so every render becomes a queued metacall — and a queued call
can arrive after its window has died, which is a SIGSEGV in
``vtkCocoaRenderWindow`` (seen in the field, 2026-08-20). ``undeferred``
removes the indirection at construction. These tests cannot reproduce
the race — it needs a real Cocoa window dying under a queued call — so
they pin the two things that close it: the helper really is synchronous,
and every plotter this application constructs goes through it.
"""

from __future__ import annotations

import ast
import pathlib
import threading

SOURCES = pathlib.Path(__file__).parent.parent / 'src' / 'hertzforge'


class _ThreadedLikePyvistaqt:
    """The shape of QtInteractor's macOS render path, in miniature."""

    def __init__(self):
        self.calls: list[str] = []
        self._rendered = False

    def _render(self):
        self.calls.append(threading.current_thread().name)

    def render(self):
        thread = threading.Thread(target=self._render)
        thread.start()
        thread.join()


def test_every_qt_plotter_this_application_builds_is_undeferred():
    """The helper only helps at the construction site, and the fixture
    windows are offscreen so no test ever runs those sites — an AST
    walk is what holds them, the way the license boundary is held."""
    makers = {'QtInteractor', 'BackgroundPlotter'}
    bare = []
    for path in SOURCES.rglob('*.py'):
        tree = ast.parse(path.read_text(encoding='utf-8'))
        wrapped = set()
        for node in ast.walk(tree):
            if (isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Name)
                    and node.func.id == 'undeferred'):
                for inner in ast.walk(node):
                    wrapped.add(id(inner))
        for node in ast.walk(tree):
            if (isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Name)
                    and node.func.id in makers
                    and id(node) not in wrapped):
                bare.append(f'{path.relative_to(SOURCES)}:{node.lineno}')
    assert not bare, ('Qt plotters built without undeferred() — each is '
                      f'a queued-render crash waiting for a dead window: '
                      f'{bare}')
