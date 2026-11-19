# Test data

The small fixtures are committed here. The plate's runs and models (55 MB) are too large for the
repository's history, so `scripts/fetch_testdata.sh` fetches them into `plate/`, which is ignored by
git. A test that needs the plate is skipped with that command named when `plate/` is absent, and CI
fetches it before running the tests.

Every model here is built by the package itself, from `hertzforge.core.fem` and
`hertzforge.demo.plate`. The sdynpy demonstration models that seeded the first fixtures are gone,
generators included.

## The plate

`plate/` holds every object type the package handles. It comes from `hertzforge.demo.plate`: the
12 in x 12 in x 0.5 in 6061-T6 free plate, whose frequencies check against the classical tables.

The set is laid out like a modal test: four fixed reference DOFs where the article would be struck
or shaken, and responses measured over the survey grid. The FRFs are every response against each of
the four drive points. The time data is one hammer hit, the impulse response at every measured DOF
obtained by inverse-transforming those FRFs, so the time data, the FRFs and the mode shapes all
describe the same structure.

```
plate/
  geometry.npz        sdynpy-format geometry (no units in the file)
  geometry.unv        the same geometry as a universal file (no 164)
  geometry.exo        the same geometry as exodus, 144 SHELL4 quads
  test_geometry.npz   the modal run's nodes with survey tracelines
  shapes.npy          22 mass-normalized modes to 5 kHz over 1014 DOFs
  frfs.npz            25 responses x 4 references (m/s^2 per N), 2 Hz
                      lines to 2.5 kHz
  frfs.unv            the same FRFs as UFF dataset 58 with a 164
                      declaring SI
  time.npz            impulse response at all 25 responses, corner drive
  psd.npz             25 autospectral densities
  spectrum.npz        linear spectra
  channel_table.vdyn  29 channels: 25 accelerometers and 4 load cells
  shock.nc4           four shocks in one recording, synthesized in
                      Rattlesnake's format (see generate_plate.py)
  modal.nc4           a Rattlesnake modal survey: 11 channels against
                      2 shakers, burst random, 20 averages
  modal_spectra.nc4   that run's FRFs and multiple coherence
  random.nc4          a Rattlesnake MIMO random test: 8 control channels,
                      4 shakers, controlled to a shaped specification
  random_spectra.nc4  that run's spectra, saved while at level: FRFs,
                      coherence, the 8x8 response and 4x4 drive
                      cross-spectral matrices, and the specification
```

The references are 101Z+, 110Z+, 1304Z+ and 1310Z+: one corner and three edge stations at the
quarter points, which are the generic positions on a square plate. Every interior survey station
lies on a center line or a diagonal, and a reference there cannot see the mode family that has a
node along that line. The first reference set had three of its four references unable to see the
647 Hz mode, and its fitted shape could not resynthesize the measurement.

Responses are Z only, because the plate lies in the XY plane and its in-plane response is numerical
noise. The modal run has one in-plane channel on purpose, because a real survey usually has a sensor
pointed where nothing moves.

The channel layout is `plate_layout.py`, so the survey the display geometry describes and the survey
the controller ran are the same survey. `tests/test_correlate.py` holds that.

Everything is written in SI. Only `frfs.unv` declares its units, through UFF dataset 164. Every other
file not written by the controller carries none, which exercises the unit-less import path and the
step of declaring units. The `.npz` files are compressed, which makes them about half the size.

## The Rattlesnake runs

The modal and random `.nc4` files came from the Rattlesnake controller running on virtual hardware
that convolves the plate's FRFs in real time, with a control law closing the loop frame by frame. The
script that drove them imports Rattlesnake and sdynpy, which are GPL, so it is not in this repository
and only its output is used here.

These are the only files that carry their own units. A Rattlesnake channel table names the
engineering unit of every channel, so they import fully defined. Each holds accelerations and forces
side by side, which is the case per-record units exist for.

A run saves twice, and the two files share nothing. Streaming writes the time histories. A separate
save writes what the environment computed from them: FRFs and coherence, and for random the cross
spectra too. Both are kept.

Two of the random run's drives are also control channels, so a drive point's force record and its
acceleration record share a DOF name. The run drives four shakers against its eight control channels.
Two shakers were tried first and could not hold the plate.

The modal test is burst random. The excitation drives for half of each frame and holds zero for the
rest, so the response rings down inside the frame it started in. The FRF window is therefore
`rectangle`, because a Hann window would discard the start of the burst, and the overlap is 0 %,
because one frame is one burst cycle.

The importer handles two things in a modal save. The saved FRF matrix has each drive against the
drives as well, since a modal environment counts every enabled channel as a response; those rows are
identity and cross-talk by construction, and they are dropped. The save also carries time data that
is one frame per accepted average laid end to end, which is split into one record per capture.

## Other committed files

`sdynpy_oracle/` holds sdynpy's own answers as data, for the oracle tests. Its `README.md` says how.

`vdyn_corpus/` holds one `.vdyn` for each older schema (1 to 3). Hertzforge reads `.vdyn` and never
writes it, so these are never regenerated. `hforge_corpus/` holds one `.hforge` for each Hertzforge
schema, written by `generate_hforge_corpus.py` when the schema changes and never refreshed after.
`tests/test_vdyn_corpus.py` and `tests/test_hforge_corpus.py` open every file in both.

The run logs follow the same rule. `vdlog_corpus/v1.vdlog` is the frozen older log, and `hflog_corpus/`
holds one `.hflog` for each run log format version, written by `generate_hflog_corpus.py`.
`tests/test_run_log.py` opens and verifies every file in both.

## Regenerating

Everything but the controller runs regenerates from this checkout through the package's own exporters:

```
python testdata/generate_plate.py            # the plate set, into testdata/plate
python testdata/generate_plate_projects.py   # three projects, into testdata/plate_projects
```

`generate_plate.py` writes the channel table as `channel_table.hforge`. The `.vdyn` in the fetched set
is older, and the tests that read it stay on it because it exercises the reader.
