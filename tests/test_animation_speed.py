"""Faster and Slower, for both kinds of animation.

A mode shape sweeps its phase and a time history walks its cursor, and
until now each did it at one fixed rate — two seconds a cycle, ten
seconds a record. Neither is right for every model: a mode that reads
well on a beam is a blur on a 400-node survey, and a long capture takes
a coffee break to watch.

The two buttons flank the transport, which is where a media player puts
them, and step a ladder of doublings rather than offering a number
nobody wants to type.
"""

from __future__ import annotations

FRAMES = 31          # not a whole number of cycles at any speed
