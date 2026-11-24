# Comparing Test and Model Time Responses

A model's predicted time responses are compared with a test's measured ones station by station:
the same DOF, the same quantity, one trace from each set. Hertzforge puts each pair on one clock,
draws the two traces over each other, and scores how well their shapes agree with the time
response assurance criterion (TRAC).

## The scores

For a pair of traces, the measured one a and the predicted one b, sampled on one clock inside the
scoring window:

- TRAC = (a·b)² / ((a·a)(b·b)). It runs from 0 to 1, and is 1 when the two traces have the same
  shape. Because the inner product is squared and normalized, a trace with its sign reversed, or
  scaled by any constant, scores the same as the original.
- The signed correlation, a·b / √((a·a)(b·b)), is negative for a sign reversal.
- The RMS ratio, ‖b‖ / ‖a‖, is the difference in amplitude.
- sTRAC, (a·b) / (b·b), is the least-squares factor that multiplies the model's trace to fit the
  test's. Its ideal value is 1. A value of -2 means the model is half the test's size and the wrong
  way round.

A trace that is zero, constant or not finite inside the window has no score, and its row in the
table says why. The summary counts the stations scored, and the mean TRAC is taken over those
alone.

## Pairing and the shared clock

Each response record of the test is paired with the model's record at the same signed DOF and the
same quantity. Force and other reference channels, and monitors such as voltages, are left out.
A DOF with no partner, or with more than two records, is refused with the DOF named instead of
being paired by guessing. Records in two different quantities, or with units not yet declared,
are never paired. Pick records under one of the two histories in the tree to compare only those
stations; the other history gives their partners.

The pairs are put on the coarsest of the two clocks, inside the time both cover, and the other
trace is interpolated linearly onto it. Nothing else is done to them: no lag is removed, no sign
or gain is fitted, and no filter is applied. A source sampled much faster than the shared clock
is interpolated without an anti-alias filter, so filter it first (Filter, then Apply Filter) if
it holds content above the coarser clock's Nyquist frequency. A model whose response lags the
test's by a few milliseconds scores low, and the overlay shows the lag.

## In the app

Pick the test's time history, then TRAC Overlay in the column beside the model's. The form opens
on its preview: the score table, one row a station with its TRAC, sTRAC, correlation and RMS
ratio; the first station's two traces, the test solid and the model dashed, with the scoring
window marked; and the summary. Pick a row in the table to draw that station. Turn off Score the
whole shared clock to give the window as TRAC start and TRAC end, in seconds.

Apply adds a Time Response Comparison to the project: the paired traces on their shared clock,
with the pairs, the sources and the window kept with it. Pick it in the tree and the same reading
comes up; pick records under it to draw those stations together. Scoring Window keeps a new
window, typed or dragged by its edges on the plot. The comparison is derived from its two
sources, so it is marked out of date when either one changes and Recompute works it out again
from the same pairs, keeping its window; it follows a source that is renamed.

## From a script

```python
import hertzforge
from hertzforge.core.time_compare import scores

project = hertzforge.Project.open('test.hforge')
name = project.compare_time_histories('Test Responses', 'Model Responses', start=0.1, stop=0.6)
project.set_time_comparison_window(name, 0.2, 0.6)
for row in scores(project[name]):
    print(row['dof'], row['trac'], row['strac'], row['reason'])
```

`compare_time_histories` takes the sources with the test's first, `pairs=` as a list of
`[first name, record, second name, record]` for pairs made by hand (they may join two DOFs, never
two quantities), and `records=` to limit the automatic pairing to some records of each source.
`scores` reads a comparison over its own window, or over `start` and `stop` when they are given.
