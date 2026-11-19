# A Reference From Field Data

**Status: a proposal for the analysis service's maintainer.** Nothing here is built yet. It states
what a tool in the analysis service would do to turn field or flight records into the reference
spectral density matrix a multiple-input, multiple-output (MIMO) random test controls to, and what
it hands the controller. The interface and the screens are for the analysis maintainer to decide.

## What It Produces

One native reference file (`docs/reference-file.md`): the spectral density matrix (SDM) per line
on the controller's grid, the alarm and abort lines per control, the band, the control-space labels,
and a provenance record with `method: field_data`. The analysis service already writes that file
from `.mat` and `.hforge` specifications (`hertzforge.io.reference_convert`); this tool would be one
more source for the same writer. The controller reads the file and runs its own validation, so
anything this tool gets wrong is refused there by name rather than run.

## Steps

### 1. Records and Channels

- Read every record with the readers the analysis service already has.
- Choose the channels that become controls, in the order the test will use. With a response
  transformation (for example the six rigid-body degrees of freedom), the SDM is formed on the
  channels and then mapped, `G_v = T_r G T_r^H`, or formed from the transformed records directly.
- Bring every record to one unit per channel and one sample rate (resample where they differ),
  and state both in the provenance.

### 2. Estimating the SDM of One Record

- Welch's method, with the frame size, window, overlap and number of averages chosen by the user
  and recorded.
- One-sided scaling, `G = 2 / (fs * sum(w^2) * K) * sum_k X_k X_k^H` on the interior lines, so the
  integral of a PSD is the signal's variance. A Welch SDM is Hermitian and positive semi-definite
  by construction, with rank at most the number of averages `K`: with fewer averages than
  channels it is singular, which the controller accepts but which leaves no room for estimation
  error, so `K` should be several times the channel count.
- **Convention.** The controller writes `G = E[X X^H]`. The usual scientific-Python estimator
  (`scipy.signal.csd(x, y)`, which forms `conj(X) Y`) gives the complex conjugate. The tool
  must conjugate before writing, and a test with a known non-zero cross-spectral phase must pin
  the sign: a channel that lags another by a positive angle appears with a positive phase in
  `G_lead,lag`.
- The line spacing must equal the controller's grid spacing, or divide it exactly so the lines
  can be picked; the controller does not interpolate.

### 3. Combining Several Records

| Method | Effect on definiteness | Use |
|---|---|---|
| Average of the SDMs (any positive weights) | Preserved: a convex combination of positive semi-definite matrices is positive semi-definite | The default, for records of one environment |
| Envelope of the autospectra, with the averaged coherence and phase | Preserved, by rebuilding `G = D C D` from the enveloped diagonal `D` and the averaged coherency `C` | Conservative references from records of differing severity |
| Percentile per line of the autospectra (for example P95), with the averaged coherence and phase | Preserved, by the same rebuild | A statistical upper bound when many records exist |
| Element-wise maximum of every entry | **Not** preserved | Not offered |

The rebuild: from the averaged SDM `G_avg`, the coherency `C_ij = G_avg,ij / sqrt(G_avg,ii G_avg,jj)`;
from the chosen autospectra `S_i`, `D = diag(sqrt(S_i))`; the reference is `D C D`. A positive
diagonal congruence keeps `C`'s definiteness, so the reference is positive semi-definite with
exactly the chosen autospectra.

### 4. Enforcing Positive Semi-Definiteness

Smoothing, enveloping by hand, editing or rounding can still break definiteness. Before writing:

- force each line Hermitian, `G = (G + G^H) / 2`;
- compute the smallest eigenvalue per line;
- where it is below `-1e-6` of the line's largest, clamp the negative eigenvalues to zero and
  rescale the diagonal back to the chosen autospectra (the same repair the controller offers);
- report the lines repaired and each pair's coherence before and after, and record the repair in
  the provenance.

### 5. Smoothing and Band

- Optional smoothing per line (a running mean over a stated number of lines, or fractional-octave
  averaging of the autospectra with the coherency kept), recorded in the provenance.
- The band is the frequency range the test controls. Lines outside it are written as zero, every
  autospectrum inside it must be positive, and a channel with no energy in part of the band must
  be raised to a floor the user states or the band narrowed.

### 6. Tolerances

- Alarm and abort lines per control as dB above and below the reference autospectrum, by default
  plus and minus 3 dB for alarm and 6 dB for abort, the same defaults as the breakpoint editor.
- Any wider band a test standard sets is entered by the user; the tool does not invent one.

### 7. Provenance

The record the tool writes into the reference file:

| Field | Content |
|---|---|
| `method` | `field_data` |
| `source` | Every record's file name and the channels taken from it |
| `source_sha256` | The hash of the first record, or of a manifest listing every record's hash |
| `assumptions` | Units, resampling, frame size, window, overlap, averages, combination method, smoothing, floors, and the conjugation |
| `repair` | What the definiteness repair changed, when it ran |
| `tool` | The analysis service and its version |

## Sources

- Hunter, Cross and Nelson (2018): a field SDM computed from Fourier spectra is positive
  semi-definite, but noisy, modified or enveloped data easily is not; the eigenvalue clamp repair.
- Smallwood (1998): the specification form of autospectra, coherence and phase.
- Jacobs et al. (2016): methods for deriving a laboratory input from field responses, compared on
  predicted error.
- Higham (1988): the nearest positive semi-definite matrix in the Frobenius norm clamps the
  negative eigenvalues to zero.
