"""Every act lives on the bar, and nowhere else.

An act with no settings — Integrate, Transform, Merge, Generate Report,
Recompute — is a push button on the bar of the pane showing what it
changes, labeled, in its own fenced group; a reading with settings
keeps its toggle and its pane. The bar follows the selection: one
object's own acts, or the acts a combination can take and nothing that
applies to one of its members alone. The tree's column carries the
refresh badge only, and the right-click no computation (Brandon,
2026-09-04: the three homes an act used to have confused a user, and
one rule replaces them).
"""

from __future__ import annotations

import numpy as np

T = np.arange(4096) / 2048.0

#: the verbs whose act is the apply button of a reading's pane — the
#: four parts of principle 13, so not a bar button of their own
PANE_VERBS = {'filter_data', 'truncate_data', 'resample', 'detect_shocks',
              'compute_spectra', 'compute_psds', 'compute_cpsds',
              'compute_srs', 'compute_frfs', 'compute_multiple_coherence',
              'compute_octave', 'generate_rigid_body_modes',
              'solve_modes', 'author_specification', 'make_realizable'}

#: the comparison table's own act: two sets are matched where they
#: are compared, pair by pair
TABLE_VERBS = {'match_modes'}
