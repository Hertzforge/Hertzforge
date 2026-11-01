"""File → Check for Updates: the answer in words, never an action.

The window asks `update.attempt` off the main thread and says one of
three things: a newer version is available (with the page to open),
this one is up to date, or **why** it could not ask — offline is not
the same news as current, and a proxy is not the same news as an
unverifiable certificate. Nothing is downloaded and nothing runs;
`update.py` explains why.
"""

from __future__ import annotations


def _distribution(direct_url):
    class Distribution:
        def read_text(self, name):
            return direct_url if name == 'direct_url.json' else None
    return Distribution()
