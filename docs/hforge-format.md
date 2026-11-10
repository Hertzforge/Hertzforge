# The .hforge File, Laid Out

A `.hforge` file is a standard HDF5 file with a documented layout inside;
the extension does not mean a new container. Rename one to `.h5` and
HDFView browses it, `h5py` walks it, and MATLAB's `h5read` reads every
dataset. This page writes the layout down so that a file can be read
without reading the source. In the code the layout is defined in
`hertzforge.io.native` (the breakpoint table files in
`hertzforge.io.tables`), and
`tests/test_hforge_corpus.py` holds the code to it with the frozen files
in `testdata/hforge_corpus/`.

The layout started as the `.vdyn` schema 3, and every
object is stored the way `.vdyn` stores it. Three things differ: the
root stamp, the root attribute that holds the object groups, and the
names of a geometry's element-group fields. Hertzforge also opens every
`.vdyn` file and saves it as `.hforge`, so an older project
moves across by opening it and saving it.

## The Version Number

Every file stamps one root attribute:

    hertzforge_schema = 1 or 2   (integer)

Schema 1 holds everything `.vdyn` schema 3 held. Schema 2 adds objects
held by reference to another file (below), and only a file with such an
object is stamped 2.

The schema changes by addition. New datasets and attributes are
optional, with a default when they are absent, and existing ones are
never renamed or given a new meaning. A file is stamped with the
earliest version that holds everything in it, so an older reader still
opens it. The reader follows three rules:

- a stamp newer than the reader knows is refused with a message saying
  the file was written by a newer Hertzforge and to update to open it;
  the file is never half loaded;
- a file with neither this stamp nor the `.vdyn` stamp
  is refused as not a Hertzforge file, since every writer stamps;
- an older stamp always loads.

While Hertzforge is an alpha this is a practice, not a promise: the
format may still change, and the promise that a later version opens
every earlier file starts with the first release that is not an alpha.

## Two Kinds of File

A single object saved on its own is one group at the root, named for
its kind: `geometry/`, `data/`, `shapes/`, `report/`, `photos/`,
`matches/`, `channel_table/`, `sine_specification/`, `sine_levels/`,
`predictions/` or `run_log_extract/`. The loader recognizes the file by
the group it finds.

A breakpoint table file is a single-object file too, holding one of a
test's tables rather than an object a project holds: `breakpoint_table/`
(below). It is opened from the table it belongs to; opening or importing
one as a project is refused, saying so. A test's channel table is the
`channel_table/` object, saved on its own like any other.

A whole project is an `objects/` container of numbered groups (`0000`,
`0001`, and so on, which keep the order). Each group has two
attributes, `name` (the object's name, which may contain anything HDF5
would otherwise read as a path) and `kind` (one of the group names
above), and holds the object's own layout. The project's identity is in
root attributes:

    test_name         the project's name
    project_type      '' or e.g. 'Modal Test', 'Random Vibration'
    active_geometry   '' or the name unlinked objects are read against
    object_groups     JSON, the project's object groups: [{"members":
                      [names...], "role": null, "Basis" or "FEM",
                      "name": ...}, ...]; "name" is present only for a
                      named group
    provenance        JSON: {name: {"verb", "source", "params",
                      "state"}, ...}, how each derived object was
                      computed, with the settings fingerprint the
                      staleness check compares against. Optional key
                      "source_print", the digest of the source's values
                      when the fingerprint holds settings only; absent,
                      only the settings are compared

A project whose data index holds anything also has one dataset at the
root (absent otherwise):

    data_index        JSON, one string: [{"name", "path", "size",
                      "sha256", "holds", ...}, ...], every file the
                      project rests on, in the order indexed. Optional
                      keys: "format", "role" ("referenced" or
                      "attached"), "source" (what added the entry),
                      "objects" (the names read from the file),
                      "indexed", "status" ("matches", "changed" or
                      "not found"), "checked" and "found" (the last
                      check), and "text" (an attached text file's
                      contents). A dataset rather than an attribute
                      because an attached file's text can pass the
                      64 KiB an HDF5 attribute holds

## Objects Held by Reference

A project can hold an object by reference to a `.hforge` file instead
of a copy, so a project that analyzes a long run does not store the run
a second time. Hertzforge does this for every object of 64 MiB or more
imported from another `.hforge` file, as long as the object has not
been edited since it was read. An edited object is saved whole.

A referenced object is a numbered group under `objects/` with its
`name` and `kind` like any other, no datasets, and these attributes:

    reference_path         the file, relative to the folder the project
                           file is in, with '/' between folders; the
                           absolute path when there is no relative one
                           (another drive, on Windows)
    reference_absolute     the file's absolute path when it was saved
    reference_object       the object's name in that file; '' when the
                           file holds a single object (the root group
                           named for `kind`)
    reference_size         the file's size in bytes
    reference_sha256       the file's SHA-256, 64 lower-case hex digits
    reference_modified_ns  the file's modification time, in nanoseconds

The reader looks for the file by its relative path, then its absolute
path, then by its name in the project's folder. A file with the recorded
size and modification time is taken as it is; one with another time is
hashed, and taken when the SHA-256 matches, since a copied or moved file
keeps its bytes. The object is then read from that file, its large
arrays mapped as they would be from the project itself. When the file
is not found, or every candidate's checksum differs, the project opens
without the object, and saving it keeps the reference unchanged, so the
object comes back once the file is in place again. A referenced file
that holds the object by reference in turn is followed, up to eight
files deep.

HDF5's external links are not used, because they break with no error
when a file moves and carry no checksum. The file is also recorded in
the project's `data_index`, as every imported file is.

## Conventions

- Strings are UTF-8 (`h5py.string_dtype('utf-8')`), one dataset
  per field and one element per record. `''` means undefined: an
  ordinate unit of `''` is a channel whose units were never declared,
  and it reloads as undefined, not as dimensionless.
- Ragged arrays (element connectivity) are two datasets:
  `<name>_flat`, the values run together, and `<name>_offsets`, one
  longer than the count, so entry *i* is
  `flat[offsets[i]:offsets[i+1]]`.
- An optional field is left out when it is not set, with no
  placeholder value. A time history with no averaging has no
  `averaging_*` attributes, and a shape set that never carried complex
  modal damping has no `modal_damping` dataset.
- Numeric datasets are whatever numpy held (float64, complex128,
  int64) at full precision. Nothing is narrowed.
- Arrays are stored uncompressed unless the save asked for compression
  (`Project.save(path, compress=True)`). Then each numeric array of 64 KB
  or more is chunked along its last axis (time, for a time history) and
  stored with gzip at level 1 and the shuffle filter. Every HDF5 build
  reads both, so nothing about reading the file changes.
- A cross term is stored as conj(X)·Y, as Bendat and Piersol write it.
  The controller and sdynpy hold X·conj(Y), its elementwise conjugate,
  so the phase of one is the reverse of the other. The Rattlesnake
  specification file, the controller's `.nc4` and the sdynpy data array
  file are converted on the way in and on the way out. Every other
  format that can hold a cross term (the universal file, MATLAB, ADF,
  ESCDF) has the values as they are stored here, because none of them
  says which form it holds. A project saved before this conversion was
  added (2026-11-04) keeps a CPSD or specification imported from those
  files in the controller's form, and nothing in the file marks it, so
  import it again to correct it. A specification `.npz` exported before
  that date reads back conjugated, for the same reason.

## `geometry/`

    attrs   dimension, length_unit ('' = undeclared)
            from_deck and mass_unit, for a geometry read from a
              Nastran deck (only when set)
            mass_point, and mass and inertia when the rigid-body modes
              are to be mass normalized (only when set)
            view_eye, view_up      the view it opens on (only when set)
            solids                 JSON, the solids as their numbers
                                   were typed (only when there are any)
            mates                  JSON, the mated surfaces (only when
                                   there are any)
            group_solids           JSON, {element group id: the solids it
                                   was meshed from} (only when there are
                                   any); a group absent is found by its name
    dsets   node_id, node_xyz, node_def_cs, node_disp_cs, node_color,
            cs_id, cs_name, cs_type, cs_matrix,
            elem_id, elem_type, elem_color, elem_conn_flat / _offsets,
            elem_group         the element group of each element
            group_id, group_name   the element groups
    group_properties/<group id>/   (only element groups given properties)
            attrs   material_name, youngs_modulus, density, poissons_ratio,
                    modulus_of_rigidity (when set), thickness (plates) or
                    section_name + section [area, iy, iz, j] (beams),
                    and section_shape + section_dimensions when the
                    section was built from a shape (meters, in the
                    constructor's order),
                    orientation (beams, when set);
                    mass alone, kg, for a group of point masses;
                    stiffness, six values with NaN where a direction is
                    free, for a group of springs;
                    ground, the directions held ('X Y Z RX RY RZ'), for
                    a group of ground points

A `.vdyn` file writes `elem_group`, `group_id`, `group_name` and
`group_properties` as `elem_block`, `block_id`, `block_name` and
`block_properties`, and the object groups as `links`. The `.vdyn` reader
takes care of those, and of the tracelines in the oldest of those files; none of them appear in a `.hforge` file.

## `data/`, Every Measurement Class

One layout serves TimeHistory, Spectrum, Frf, Psd, Srs, Coherence,
MultipleCoherence, Specification, ShockSpecification,
TransientSpecification, SineLevel and SineTarget. The concrete class is
stored by name, because a Specification has the PSD function type and
the function type alone cannot say which class to rebuild:

    attrs   function_type        UFF dataset 58 code
            data_class           the class name, e.g. 'Psd'
    dsets   abscissa             one shared axis
            ordinate             records × points, real or complex
            response_dof         strings, one per record
            ordinate_dim, comment, ordinate_unit, reference_unit,
            dimension_hint       strings, '' = undefined / not stated

Present only where they mean something:

    dsets   reference_dof     records that have a reference (FRF, coherence)
            block             which repeat a record is, for stacked captures
            limit_<name>      a specification's warning and abort lines
            bandwidth         band widths, where the bins are not the
                              midpoints between lines (octave bands)
            statistical_dof   a PSD's statistical degrees of freedom,
                              one per record, where it was computed
                              from a time history
            line_correlation  where it was computed from a time history,
                              how its window ties neighboring lines:
                              the correlation at 0, 1, 2, ... lines
                              apart, starting at 1, one for every record
            shocks            (start, duration) rows, where events were marked
            segments, segment_kinds, segment_names   (start, end) rows,
                              where segments were marked, with each
                              mark's kind and name (strings, '' = unnamed)
            roles             strings, DOF, quantity and role in threes,
                              flat: each channel's role (reference,
                              response or monitor) where something has
                              set one. A channel not listed, or a file
                              without the dataset, takes the default for
                              its quantity (a force is a reference, a
                              motion a response, anything else a monitor)
            sine_seconds      a sine level's clock
            sine_floor, sine_below_floor   the noise floor beside each
                              sine reading, and where the tone was under it
    attrs   srs_q, srs_kind          what an SRS was computed with
            interpolation            how a specification reads between
                                     its lines
            band_constraints         JSON, how a specification's bands
                                     move together when edited
            sine_tone, sine_onset, sine_drift_hz   which tone a sine
                                     level belongs to, and where its
                                     sweep began
            sine_cycles (when set), sine_target_db, sine_refine,
            sine_chosen (when set)   the sine extraction setting
            source                   'SIM' for values from a simulated
                                     plant, 'HARDWARE' for values measured
                                     on real hardware; absent when not
                                     known. A Rattlesnake run sets it from
                                     its hardware, and what a project
                                     derives from it carries the same
            scale_db                 a comparison scale set by hand only;
                                     automatic scaling is never saved
            narrowbands              JSON, the narrow bands of random a
                                     mixed test lays over this floor
                                     (each a center, a width, a level and
                                     a sweep), when it has any
            tone_exclusion           the width, Hz, left out of this
                                     floor either side of a mixed test's
                                     tones, when set
            run_identity             JSON, a time history's run: axis,
                                     run, level_db, start and comment
                                     where known, "sources" (where each
                                     came from) and "refused" (what was
                                     not read, and why); absent when
                                     nothing has said. The level the
                                     comparison detects is never saved
            averaging_frame_length, _overlap, _window, _frames, _start,
            _detrend, _start_set, and _window_parameter when the window
            takes one
            filtering_low, _high (each when set), _order   the filter
                                     reading
            truncation_start, _stop  the truncate reading, when set

## `shapes/`

    attrs   mass_unit ('' = undeclared), unscaled (no drive point, so
            the modal masses are a convention, not a measurement)
    dsets   frequency, damping, shape_matrix (modes × DOFs, complex
            where a source carried complex modes), modal_mass
            (complex where measured), coordinate, comment, description
            modal_damping        only where a source carried one

## `channel_table/`

One table for a test and its analysis: what the analysis reads (each
channel's DOF, role, sensor and calibration) and, when the table has
been converted to carry them, the settings a controller runs each
channel at.

    attrs   column_order         every column, in the file's order
    dsets   channel              integers (the schema's one 'int' column)
            <every other column> strings, verbatim (node included: a
                                 node nobody recorded is blank, since 0
                                 cannot stand for "not stated")
    environments/<k>/            the control loops, when there are any,
                                 numbered because an environment's name
                                 is free text and may hold a slash
            attrs   name
            dsets   controls, drives   channel numbers
                    response_transformation, reference_transformation
                                       matrices, when set

Columns are read by the type they were stored as, so a column the
schema has since dropped still reads.

The columns are `channel`, `node`, `direction`, `role`, `control`,
`channel_type`, `unit`, `sensitivity` (mV per unit), `daq_range`,
`sensor_range` (volts), `serial_number`, `make`, `model`, `triax_dof`,
`expiration` and `comment`.

The controller's settings are an optional group of columns after them,
all present or none: `enabled` ('True' or 'False'), `drive` ('True' for
an output to a shaker amplifier), `physical` (the hardware channel,
'<device>/<channel>', blank when unbound), `iepe` ('True' for an IEPE
input) and `iepe_bias` (volts), `polarity` ('positive' or 'negative',
blank for positive), `range_max` (the input's full scale in the
channel's unit), `control_group`, and the abort limits `abort_accel_g`,
`abort_velocity_ips` and `abort_displacement_mil`, blank where none is
set. Their presence in `column_order` is the record that the table was
converted to carry them; a table that never was has none of them and
reads exactly as before. A channel's role in a control loop is read
from three columns: a drive is `drive`, a control channel is `control`,
a reference is a `role` of reference, and any other input is a monitor.
Converting a table adds the group at its defaults: every channel on,
none a drive, each full scale its clip level in volts over its
sensitivity where both are stated, and the rest blank.

## `breakpoint_table/`

One breakpoint table behind a test's profile, target or limits.

    attrs   column_order         the columns, in the file's order
            table_kind           which table it is: 'random_profile',
                                 'sine_profile', 'sine_limit',
                                 'srs_target', 'limit' or 'drive_gain'
            x_unit, y_unit       the units of the x column and of the
                                 levels ('' where the rows say, as a
                                 Sine profile's quantity does)
    dsets   <every column>       float64, NaN where the row has none;
                                 a text column (a Sine profile's
                                 quantity) as strings

The x column is `freq_hz`. The other columns are the table kind's: a
Random profile's `psd_g2_hz` and `slope_db_oct` (one of the two on each
row), a Sine profile's `quantity` (`accel_g`, `velocity_ips` or
`disp_pp_in`) and `value`, a Sine limit's `quantity` and `bound`, an
SRS target's `accel_g`, a limit's `ceiling`, a drive gain's `gain`
(g/V, from the drive to a limit channel), and on a profile's rows the
bands `alarm_below_db`, `alarm_above_db`, `abort_below_db` and
`abort_above_db`, in dB.

## `report/`, `photos/`, `matches/`

    report/   attrs: title, marking, marking_color ('ink' | 'red';
              marking '' = no strips), and when set: features (JSON
              list) and identity (JSON, the identification fields and
              the issue record);  dsets: blocks (one JSON string)
    photos/   dsets: names, formats, image_0000, ... (each photograph
              file's own bytes, verbatim, never re-encoded)
    matches/  attrs: first, second, first_geometry, second_geometry
              dsets: pairs (n × 2 mode indices), macs

## `sine_specification/`, `sine_levels/`

    sine_specification/  attrs: ordinate_dim, ordinate_unit, comment
                         dsets: response_dof, tone_order
                         tones/<k>/: attrs name, start_time; dsets
                         frequency, amplitude, phase, segment_type,
                         segment_rate, and any warning or abort limits
                         (the file's own breakpoints, per tone)
    sine_levels/         attrs: sine_cycles (when set)
                         dsets: tone_order; tones/<k>/ each an
                         ordinary data/ layout (the set only groups
                         them)

## `predictions/`

    predictions/  attrs: comment, count
                  <k>/: attrs record (one JSON string: channel,
                  quantity, unit, level_db, reference, source, customer,
                  frequency_uncertainty, level_uncertainty, and peaks,
                  each [frequency, level, Q, band] with null where not
                  stated); dsets curve_frequency, curve_values (a
                  predicted curve, when there is one)

A report's blocks may include one of kind `prediction`, naming the
predictions (`source`) and the measurement (`measured`) with the level
it was run at (`level`, dB relative to full level, or null).

## `run_log_extract/`

    run_log_extract/  attrs: path, test, upto, taken, log_size, after,
                      after_in_test, problems (a JSON list of strings)
                      dset log (one UTF-8 string: the snapshot, the
                      run log's own JSON Lines)

One test of a run log (`.hflog`, or an older `.vdlog`), frozen at
entry `upto`: `log` holds the log's header line and the lines of the
entries kept, exactly as the log file had them, so it reads with the run
log's own reader for its version and its chains verify. `path` is where
the log was, relative to the project file (with forward slashes) when
the log was in the project file's folder or one beneath it, absolute
otherwise. `taken` is when the snapshot was taken (ISO 8601 with its
offset), `log_size` the log file's size in bytes then, `after` and
`after_in_test` how many entries the file held after `upto` then, in all
and of the test, and `problems` the lines before `upto` skipped as not
entries.

## `run/`, a MIMO Random Run (Proposal)

*A proposal for the analysis maintainer, not yet part of schema 1.* The
control service records a multiple-output random run in its own folder
while it runs, and `hertzforge.io.mimo_run_convert` turns that folder
into a `.hforge` file at the end (or afterwards, for a run that did not
finish). The run is one group at the root:

    run/              attrs: test ('mimo_random'), source ('SIM' or
                      'HARDWARE'), run_folder, start_unix_secs,
                      setup_hash, complete (false for a run that did
                      not finish: what it wrote is kept, the rest left
                      out)
      settings        the run's settings, JSON text
      report          the report data, JSON text (finished runs only)
      reference/      sdm (the reference at full level), alarm_upper,
                      alarm_lower, abort_upper, abort_lower (lines by
                      controls, NaN where there is no line), controls
                      (channel indexes); attrs df_hz, band, space
      identification/ frf and g_rd (lines by responses by drives),
                      g_cc, g_dd, noise (packed), coherence (lines by
                      controls), condition (per line), drive_vrms;
                      attrs setup_hash, level_db, frames, steps,
                      lag_frames
      loops/
        diagonal/     loop_index, control_psd, control_error_db
                      (loops by lines by controls, full level, NaN out
                      of band), drive_psd (loops by lines by drives)
        summary/      loop_index, level_db, state, law_misses, and
                      json (each loop's whole summary line)
        sdm/<loop>/   response_full_level, drive_full_level (packed);
                      attrs loop_index, why (the first loop at full
                      level, the cadence, the last loop at full level)
      law/            attrs id, name, version, sha256, builtin,
                      catalog_path (a law file only); params (one JSON
                      string per parameter, `{"key","kind","value"}`),
                      parameters (as the report prints them), file (a
                      law file's bytes, uint8)
        calls/        index (one JSON string per law call: seq, call
                      'seed' or 'iterate', loop_index, response_dof,
                      df_hz, band, outcome, diagnostics, the arrays it
                      holds with their byte offsets, and the inputs
                      carried from an earlier call), payload (the
                      calls' arrays, little-endian float64, uint8);
                      attrs format

Every spectral matrix is in the controller's convention,
`G = E[X X^H]` per line, Hermitian, and stored packed: a dataset of
lines by `n (n + 1) / 2` complex values, the upper triangle row by row
(`G[0,0], G[0,1], ..., G[0,n-1], G[1,1], ...`), with attributes
`packing`, `convention` and `n`. Phases are kept as recorded; a reader
that holds the other convention conjugates at its edge. The law calls
are what a replay of the run feeds the law (acceptance A11 of the MIMO
design): the arrays in each call are the law's declared inputs and its
output drive.

## Reading One Without Hertzforge

Because the layout is documented HDF5, this works:

```python
import h5py

with h5py.File('modal.hforge') as f:
    for key in sorted(f['objects']):
        group = f['objects'][key]
        print(group.attrs['name'], group.attrs['kind'])
    frf = f['objects/0003']                    # say it's the FRF
    H = frf['ordinate'][()]                    # records x frequencies
    freq = frf['abscissa'][()]
```

MATLAB reads the same file with `h5info('modal.hforge')` and `h5read`.
Nothing in a `.hforge` file needs Hertzforge to get it back out.
