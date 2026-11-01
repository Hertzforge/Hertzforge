# The .hforge File, Laid Out

A `.hforge` file is a standard HDF5 file with a documented layout inside;
the extension does not mean a new container. Rename one to `.h5` and
HDFView browses it, `h5py` walks it, and MATLAB's `h5read` reads every
dataset. This page writes the layout down so that a file can be read
without reading the source. In the code the layout is defined in
`hertzforge.io.hforge`, and `tests/test_hforge.py` holds the code to it.

The layout started as Visual Dynamics' `.vdyn` schema 3, and every
object is stored the way `.vdyn` stores it. Three things differ: the
root stamp, the root attribute that holds the object groups, and the
names of a geometry's element-group fields. Hertzforge also opens every
`.vdyn` file and saves it as `.hforge`, so a Visual Dynamics project
moves across by opening it and saving it.

## The Version Number

Every file stamps one root attribute:

    hertzforge_schema = 1   (integer)

The schema changes by addition. New datasets and attributes are
optional, with a default when they are absent, and existing ones are
never renamed or given a new meaning. A file is stamped with the
earliest version that holds everything in it, so an older reader still
opens it. The reader follows three rules:

- a stamp newer than the reader knows is refused with a message saying
  the file was written by a newer Hertzforge and to update to open it;
  the file is never half loaded;
- a file with neither this stamp nor Visual Dynamics' `visualdynamics_schema`
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
                      staleness check compares against

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
takes care of those, and of the tracelines in files written by Visual
Dynamics 0.1.0a20 and earlier; none of them appear in a `.hforge` file.

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

    attrs   column_order         every column, in the file's order
    dsets   channel, range       integers (the schema's 'int' columns)
            <every other column> strings, verbatim (node included: a
                                 node nobody recorded is blank, since 0
                                 cannot stand for "not stated")

## `report/`, `photos/`, `matches/`

    report/   attrs: title, marking, marking_color ('ink' | 'red';
              marking '' = no strips);  dsets: blocks (one JSON string)
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

One test of a run log (`.vdlog`, Visual Dynamics' run log), frozen at
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
