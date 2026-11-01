"""Every documented workflow journals as a script that replays it.

Brandon's rule (2026-08-30): nothing in the console may be
not-replayable, by design. Each driver below walks one project type's
workflow through the window's own handlers — the acts the guide
teaches — then holds the journal to the rule and *runs* it: exec the
session script, compare the projects. The stressdata-fed drivers skip
where the local run data is absent, the pattern `test_extract_sine`
set.
"""

from __future__ import annotations

import os

STRESS = os.path.join(os.path.dirname(__file__), '..', 'stressdata',
                      'plate')
