"""A long verb's worker makes, frees and collects no Qt object.

`MainWindow._run_long` runs a verb on a worker thread while this thread
turns the event loop. Qt objects belong to the thread that made them and
are deleted there; one deleted on the worker, under the loop turning
here, crashes the loop. PySide deletes a Python-owned Qt object wherever
its last reference goes, and Python's cyclic collector runs on whichever
thread happens to allocate. So the runner leaves nothing of Qt's for the
collector, and the collector does not run on the worker at all. Both are
checked here directly, so the unsafe pattern fails on any platform, not
only where the crash shows.
"""

from __future__ import annotations

#: what `_run_long` makes for a call: other parts of the window (a plot's
#: context menu, for one) leave cycles of their own, which are not the
#: runner's and are freed on the window thread when the window closes
RUNNER_KINDS = ('_LongVerbRelay', 'QEventLoop', '_InputGate')


def _runners(found):
    return [name for name in found if name in RUNNER_KINDS]
