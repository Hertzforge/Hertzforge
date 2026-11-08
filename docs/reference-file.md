# The Reference File, Laid Out

A multiple-output random test runs to a **reference**: the spectral density matrix (SDM) the
controls are driven to, line by line, with the alarm and abort lines, the band, the names of the
controls and a record of where it came from. The controller keeps it in its own file, written and
read by the `hertzforge-reference` crate and written by the analysis service's converter
(`hertzforge.io.reference_convert`). This page is the layout, so the file can be read without
either.

## The Folder

A reference is a folder of two files, in the same family as the controller's other records: the
numbers in a little-endian binary file, and a JSON sidecar that says how to read them, written
last.

```
<name>/
  reference.bin    the content
  reference.json   the sidecar
```

## Conventions

- **Spectral matrix:** `G = E[X X^H]` per line, so a response `Y = H X` has `G_yy = H G_xx H^H`.
  This is the complex conjugate of the Bendat and Piersol form many analysis tools use
  (`scipy.signal.csd(x, y)` among them). A positive phase in `G_ij` means channel `j` lags
  channel `i`.
- **Grid:** line `k` is at `k * df_hz`, from 0 Hz.
- **Band:** the half-open line range `[lo, hi)`. Every line outside it is zero; every control has
  a positive PSD on every line inside it.
- **Units:** the SDM is in the control unit squared per hertz; the unit is stated as text.
- **Tolerances:** alarm and abort lines per control as PSDs, `NaN` where there is no line.

## `reference.bin`

Every field starts on a multiple of 8 bytes.

| Field | Type |
|---|---|
| Magic: the bytes `HFREF01` and a zero byte | 8 bytes |
| Lines, order `n`, band low, band high | 4 x u64 |
| Line spacing in Hz | f64 |
| Flags: bit 0, a response transformation follows; bit 1, a drive transformation follows | u64 |
| Unit | string |
| The `n` labels of the reference's rows and columns | `n` strings |
| Response transformation, when flagged: its lines (1, or one per grid line), rows (`n`), columns (`n_c`), then a label per column (the physical control channels) | 3 x u64, `n_c` strings |
| Drive transformation, when flagged: its lines, rows (`n_d`, the outputs), columns (`n_u`, the virtual drives), then a label per row and per column | 3 x u64, strings |
| The SDM: per line, the upper triangle row by row, `(0,0), (0,1), ..., (0,n-1), (1,1), ...` | `lines * n (n + 1) / 2` complex |
| Alarm upper, alarm lower, abort upper, abort lower: each line by line, control by control | 4 x `lines * n` f64 |
| Response transformation values, per line, row-major | complex |
| Drive transformation values, per line, row-major | complex |

A **string** is a u64 byte count, the UTF-8 bytes, and zero bytes up to the next multiple of 8. A
**complex** value is two f64, real then imaginary. Every number is little-endian. Nothing follows
the last array.

A lower-triangle element is the conjugate of its mirror; a diagonal element's imaginary part is
zero. The response transformation `T_r` maps the physical controls to the reference's virtual
degrees of freedom (`G_v = T_r G T_r^H`) and has full row rank; the drive transformation `T_d`
maps virtual drives to outputs (`G_dd = T_d G_uu T_d^H`) and has full column rank.

## The Content Hash

The SHA-256 of `reference.bin`, written `sha256:` and 64 hex digits. It covers everything that
makes the reference what it is and nothing about how it was made, so the same reference reached
two ways has one hash, and a run record or a message can name a reference by it.

## `reference.json`

The controller's snapshot envelope with the description as its payload:

```json
{
  "unix_secs": 1793750400,
  "settings_json": "",
  "marks": [],
  "payload": {
    "format": "hertzforge-reference",
    "format_version": 1,
    "binary_file": "reference.bin",
    "binary_bytes": 7992,
    "content_hash": "sha256:...",
    "convention": "G = E[X X^H] per line",
    "lines": 41, "n": 3, "df_hz": 5.0, "band": [4, 21],
    "labels": ["front", "rear", "top"],
    "unit": "g",
    "space": "physical",
    "response_transformation": null,
    "drive_transformation": null,
    "provenance": { ... }
  }
}
```

`space` is `virtual` when a response transformation is present. The two transformation fields
give `[lines, rows, columns]` or `null`. A reader checks that the binary's hash is
`content_hash` and that the payload describes the binary, and refuses the folder otherwise.

### Provenance

| Field | Content |
|---|---|
| `method` | `breakpoints`, `rigid_body`, `specification_npz`, `specification_mat`, `csv`, `hforge`, `field_data` or `direct` |
| `source` | The file name, or a description of what was entered |
| `source_sha256` | `sha256:<hex>` of the source file, or `null` |
| `created_unix_secs` | When it was made |
| `tool` | The software and version that made it |
| `conjugated_on_import` | For an import, whether every cross term was conjugated; else `null` |
| `assumptions` | The assumptions it rests on, in words |
| `repair` | What a positive semi-definite repair changed, or `null`: lines repaired, eigenvalues clamped, the worst relative eigenvalue, the largest change in a pair's squared coherence, and the thresholds |

## Where a Reference Comes From

### Breakpoints

A PSD breakpoint table per control (the single-output profile editor's tables, with their alarm and
abort dB bands) and, per pair, a squared coherence and a phase in degrees, each a constant or a
table of points joined by straight lines in frequency. A pair not given is independent, or fully
coherent with zero phase, as chosen. Each cross term is
`G_ij = sqrt(G_ii G_jj) sqrt(gamma2_ij) exp(i theta_ij)`.

For three or more controls the coherences and phases must also agree with one another: with unit
coherence, `theta_01 + theta_12` must equal `theta_02`. A reference that does not is refused with
the lines named. The repair offered clamps the negative eigenvalues of each such line to zero and
rescales the diagonal back to the PSDs entered; its report lists each line, its smallest
eigenvalue and every pair's coherence before and after.

### Rigid Body

Each control sensor's position and sensing direction, and a reference point, give the kinematic
matrix `A` (one row `[n, r x n]` per sensor, columns `X, Y, Z, RX, RY, RZ`). The response
transformation is `T_r = A^+`. At least six sensors are needed and the geometry must have rank
six; otherwise it is refused with the rank. Translations are in the sensors' unit and rotations in
that unit per unit length. Each sensor's standardized residual against the rigid fit flags a
sensor on a part that does not move rigidly.

### A Specification `.npz` File

| Key | Shape | Content |
|---|---|---|
| `cpsd` | lines by channels by channels, complex | The matrix per line |
| `f` | lines | The frequency of each line in Hz (integers accepted) |
| `warning_upper`, `warning_lower`, `abort_upper`, `abort_lower` | lines by channels, real, each optional | Limit lines on the autospectra; `NaN` is no line |

The analysis service's own writer adds `dof` (channel names, used as the labels), `held` and
`coordinate`; these are accepted. Any other key is refused. Rows and columns follow the control
channels in the test's order.

- **No interpolation.** A file frequency is placed on the grid line `round(f / df)` when it is
  within `1e-9` of the line spacing of it; otherwise the file is refused, naming the first
  frequency that is off the grid. Grid lines the file does not hold are zero, so a partial band is
  fine.
- **A coarser file.** A file at twice the grid's spacing leaves every other line inside its band at
  zero, and is refused: run at the file's spacing.
- **Convention.** The file does not say which form its cross terms hold. Rattlesnake and SDynPy
  write `X conj(Y)`, the controller's own `E[X X^H]` (`hforge-format.md`), and the analysis
  service's writer of this layout writes the same, so the file is read unchanged. A file made with
  an estimator that gives the Bendat and Piersol form (`scipy.signal.csd` does) is conjugated on
  request; either way the choice is recorded in the provenance.

### A Specification `.mat` File

The same variables with the axes transposed: `cpsd` channels by channels by lines, each limit
channels by lines. MATLAB v5 and v7.3 files are read by the analysis service's converter, which
writes the reference folder; channel names are read from a v5 file's `dof` or given to the
converter. The same matching rules apply. The convention differs: the analysis side takes a MATLAB
file's cross terms as the form it stores itself, the Bendat and Piersol `conj(X) Y`, so the converter
conjugates every cross term on the way to the controller by default and records that in the
provenance.

### A `.hforge` Specification

A `.hforge` file holding a random specification is converted by the analysis service: its matrix
is formed as the specification writer forms it (pairs the specification does not hold are zero),
its channel names become the labels, and the same rules apply.

### CSV

A PSD table:

```
frequency_hz,front,rear
20,0.01,0.04
21,0.01,0.04
```

and, optionally, a pair table, one row per pair per line:

```
frequency_hz,row,column,coherence,phase_deg
21,front,rear,0.5,30
```

The coherence is squared coherence (0 to 1), and the phase is that of `G_row,column` in the
controller's own convention, so nothing is conjugated. A pair not listed on a line takes the
import's choice (independent, or fully coherent with zero phase). Commas separate the cells (tabs
when the header has tabs and no commas); labels hold no commas; blank lines and lines beginning with
`#` are skipped. Frequencies follow the `.npz` matching rule. Alarm and abort lines are dB bands
about each PSD, by default 3 dB and 6 dB each way.

### From Field Data

A proposal for the analysis service: `docs/reference-from-field-data.md`.

## Validation

Every route ends in the same checks, and the controller runs them again when it reads a file:
every value finite and every dimension consistent, labels present and distinct, an imported full
matrix Hermitian within a relative error of `1e-9`, every line positive semi-definite (no
eigenvalue below `-1e-6` of the line's largest), a positive diagonal inside the band and zeros
outside, each lower tolerance line under its upper one with the abort lines outside the alarm lines
and the reference inside its own alarm lines, and transformations of full rank. Each refusal names
what failed and what to do.
