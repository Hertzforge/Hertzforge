# Modal Test Analysis

A Modal Test run opens in the analysis side as every run does: open the run, or its project
file, and the run is converted (`hertzforge.io.run_convert`). This page says what the run
becomes, how a roving hammer survey reaches the modal fit, what the impact windows do to
the fitted damping, and how a measurement is estimated again.

## What A Run Becomes

Each result of the run folder becomes an object in the project, all in one object group
named for the run. Every name the converter reads from the folder is in one table,
`hertzforge.io.modal_run.WIRE`.

| In the run folder | In the project | How |
| --- | --- | --- |
| `table.json` | **Channel Table** | Each row's node, direction and analysis role, as for every run. |
| `results/modal_frf` | **modal_frf** (Frf) | A record per response against each reference of every measurement, at the DOFs the measurement named (node and direction, written `1Z+`), in SI from the channel rows' units. A point measured again replaces the earlier measurement of it. Each record's comment says the estimator, the averages, the measurement and point, and the windows. |
| `results/modal_frf`, excitation roving | **modal_frf fixed reference** (Frf) | The same FRFs turned round by reciprocity, every record marked "transposed by reciprocity". Made when the run has more reference DOFs than response DOFs, as a roving hammer survey does. |
| `results/modal_coherence`, one reference | **modal_coherence** (Coherence), and **modal_coherence fixed reference** | The ordinary coherence of each record, and turned round as well when the FRFs are. |
| `results/modal_coherence`, several references | **modal_coherence** (MultipleCoherence) | Each response's multiple coherence. |
| `results/modal_reference_spectra` | **modal_reference_spectra** (Psd) | Each measurement's reference autospectra, in SI. With several references, the condition number of their spectral matrix stays in the run record as an array. |
| `streams/inputs`, `results/modal_frames` | **Measurement k frames** (TimeHistory) | Each measurement's accepted frames, a record per channel per frame, labeled `hit 1`, `hit 2` (or `frame 1` for a shaker), timed from the trigger so the pre-trigger samples are at negative times. A measurement replaced by a later one of its point is kept, named "(replaced)". |
| `results/modal_frames`, `results/modal_outcome` | **Run Record** | Every frame line (accepted, rejected and trial, with the reason and the checks) and the outcome (the points planned and measured, the frames, how the test ended). |

The streams are not copied whole. One time history per measurement, rather than per frame,
keeps a survey of a hundred points at five hits to a hundred objects, and the frames of one
measurement share their DOFs and their length, which is what estimating them again needs.
Rejected and trial frames stay in the stream and in the frame lines.

The window record travels on the FRF, in each record's comment: the force window, the
exponential window's time constant, the estimator and the averages, all of which survive a
save. A fit reads the time constant back from there. What the run lacked is listed in the run record:
points planned but not measured, values that are not finite numbers, a result whose units
do not match the channel table (kept as arrays, with the reason). A run ended early, or
killed, converts with what it has.

## The Fixed-Reference Form

A modal fit needs every response against every reference. A roving hammer survey measures
the other way round: one or a few fixed accelerometers, each against a force at every point
the hammer visits. By reciprocity the motion at j for a force at k equals the motion at k
for a force at j, so the set can be turned round: the fixed accelerometer becomes the
reference and the hammered points become the responses
(`hertzforge.core.modal_test.fixed_reference`).

This is valid only when every record is a translational motion (acceleration, velocity or
displacement) per force, when both DOFs of every record name a direction, and when the
structure is linear and reciprocal (symmetric mass, stiffness and damping, as a passive
structure is; a spinning, actively controlled or aeroelastically loaded one need not be).
The records cannot show the last of these: it is the test's assumption, and each turned
record says it was turned. Anything else is refused, with the reason.

A survey taken as several runs merges into one set (Merge), before or after turning.

## The Impact Windows

The force window holds one over the pre-trigger and the pulse, from the frame's trigger
sample, then falls to zero by a steep half-cosine and stays zero for the rest of the frame.
Its width is the pulse allowance after the trigger, given as a fraction of the block after
the trigger; the fall is a tenth of the width, at least one sample. A window that would run
past the block is refused rather than cut short, and a window that removes the pulse is
refused with that cause named. The exponential window, w(t) = exp(-t / tau) from the frame's
first sample, is applied to the force and every response alike. Both are an impact capture's
windows: the averaging of a continuous record does not offer them.

## The Exponential Window's Damping

An impact response windowed by exp(-t / tau) decays faster by 1 / tau: each mode's pole
moves left by exactly 1 / tau, so a fit of the windowed FRF reads a damping ratio high by
1 / (tau omega_n) to first order, where omega_n is the mode's natural frequency in radians
per second. A fit published from an FRF estimated with an exponential window notes this on
each mode, in the mode's comment: the time constant, the damping it added, and the mode's
frequency and damping less it. The fitted values are left as the fit made them, so the shape
set still synthesizes the FRF it was fitted to.

The first-order correction, zeta less 1 / (tau omega_n), is for light damping. It can be
badly off when the damping is very small and poorly estimated, since the window's share is
then most of what was fitted. Capturing more of the decay (a longer frame, a finer line
spacing) is better than correcting for a short time constant.

## Estimating A Measurement Again

Estimate Again (`project.reprocess_modal_test`) takes one measurement's frames and estimates
its FRFs and coherence again with the estimator (H1, H2 or Hv), the force window on the
references, the exponential window on every channel, or a shaker window, the block and the
frames chosen. The results are added beside the frames; the run's own FRFs are not changed.
A dead reference, a window that removes the pulse, a block longer than the frames, too few
frames for the references and frames that are not finite are refused.

## References

- Trethewey, M. W., and Cafeo, J. A. (1992). "Tutorial: Signal Processing Aspects of
  Structural Impact Testing." *International Journal of Analytical and Experimental Modal
  Analysis* 7(2), 129-149. The exponential window (equation 22) and the damping correction
  (equation 27), with its caution for very light damping.
- Fladung, W. A. (1997). "Windows Used for Impact Testing." *Proceedings of IMAC XV*. The
  force window, and the exponential window applied to the force and the response alike.
