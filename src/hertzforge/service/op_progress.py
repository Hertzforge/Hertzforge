"""Progress through an operation, and its cancel (Brandon, 2026-11-11: a long solve
showed no progress and could not be stopped, which the earlier toolset could).

The core's long verbs report to a `(done, total)` callable and stop when it raises
(`core.progress`). The server sets one for each operation it runs, on the worker thread
that runs it; an operation hands it to its verb with `hook()`. The callable sends
`op.progress` events and raises `Cancelled` once `op.cancel` has been asked for."""

from __future__ import annotations

import contextvars
from collections.abc import Callable
from typing import Any

_HOOK: contextvars.ContextVar[Callable[[int, int], None] | None] = contextvars.ContextVar(
    'op_progress', default=None)


def running_with(report: Callable[[int, int], None] | None, call: Callable[..., Any],
                 *args: Any) -> Any:
    """`call(*args)` with `report` as the progress of the operation it runs. Called on the
    thread `call` runs on, so nothing depends on a context being carried to a worker."""
    token = _HOOK.set(report)
    try:
        return call(*args)
    finally:
        _HOOK.reset(token)


def hook() -> Callable[[int, int], None] | None:
    """The running operation's progress, for its verb's `progress`; None outside one."""
    return _HOOK.get()
