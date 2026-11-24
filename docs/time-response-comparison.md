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

## Filtering or rescaling both sets first

Filter and Rescale take several time histories picked together. The form previews the first one,
and Apply makes a filtered or rescaled copy of each with the same settings. Every history is
checked before any copy is added, so if one cannot be filtered (an edge above its Nyquist
frequency, a sample that is not a number) or rescaled, nothing is added and the message names the
history. The sources keep their values. The filter is stored on each source as its own setting,
and each copy refreshes from its own source. Rescale takes the records picked under each history,
or all of them when none are picked; Filter always takes every record.

The copies go into a new group of their own, named after the sources' group: the histories of
`test.unv` filtered go into `test.unv Filtered`. Histories that share no named group go into
`Filtered 1`, or `Rescaled 1`, and the next free number after that. So to filter a test file and a
model file the same way and compare them, pick the test file's histories and apply Filter, do the
same for the model file's with the same settings, then pick every history of the two new groups
and use TRAC Overlay.

## In the app

Pick the test's time history, then TRAC Overlay in the column beside the model's. A test and a
model exported one channel to a history, as some programs write a Universal file, are compared by
their groups: an imported file's histories share a group named for the file, so pick every history
of the two files together and TRAC Overlay pairs them across both sets. The group listed first in
the tree is the test; Swap test and model makes the other one the test, which changes sTRAC and the
RMS ratio and leaves TRAC alone. The form opens
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

`project.time_history_sets(*names)` splits histories picked together into the two sets, by
their groups, as the action table does. `project.filter_histories(*names, filtering=...)` and
`project.rescale_histories(*names, factor=..., records=...)` do what Filter and Rescale do with
several histories picked, and return the copies' names:

```python
from hertzforge.core.filters import Filtering

low_pass = Filtering(order=4, high=500.0)
tests = project.filter_histories(*project.object_group_of('Test 1X+'), filtering=low_pass)
models = project.filter_histories(*project.object_group_of('Model 1X+'), filtering=low_pass)
name = project.compare_time_histories(*tests, *models)
```

`compare_time_histories` takes the sources with the test's first, `pairs=` as a list of
`[first name, record, second name, record]` for pairs made by hand (they may join two DOFs, never
two quantities), and `records=` to limit the automatic pairing to some records of each source.
`scores` reads a comparison over its own window, or over `start` and `stop` when they are given.

## Traces exported in pairs

Some test programs export each measurement to a UNV file as two time traces (dataset 58), one
whose ID line reads Response Time Trace and one that reads Reference Time Trace. Both headers
carry the measurement's DOF as the response and the reference DOF in the reference field.
Hertzforge reads each such pair as a time history of its own, named for both DOFs
(`Time History - 730Z- - ref 1013Z+`): the response channel at the response DOF and the reference
channel at the reference DOF, with their roles set. The histories of one file go in one group
named for the file, so importing the test's file and the model's gives two groups whose
histories are compared with each other.

Some exporters write a channel number in the reference node field instead of the node. When a
file's pairs name their reference at one node, the import shows the reference the file gives and
asks which node it was measured at; the direction stays as the file has it, and the number it
opens with keeps the file's node. Cancel leaves that file out and goes on with the next. A file
with no labeled pairs asks nothing, and neither does one whose pairs name references at several
nodes. A trace without its partner, or without a reference DOF, refuses the file, naming the
trace and its place among the file's function records.

From a script the answer is an argument, and without it the file's own node stands:

```python
project.import_file('test.unv', reference_node=1013)
```
