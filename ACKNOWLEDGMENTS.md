# Acknowledgments

**Thanks, not authorship.** Nothing on this page confers a copyright interest, an authorship
claim, or any right in Hertzforge. It is a record of help gratefully received and of the work
Hertzforge is built on: advice, review, test data, papers, and the projects that showed what these
tools should do.

Where the rights actually live is written down elsewhere, and the records are kept apart on
purpose:

| Record | What it means | Where |
| --- | --- | --- |
| This page | Thanks and provenance, freely given, at the authors' discretion | Here, and on hertzforge.com/credits.html |
| The terms of the work itself, and of every contribution to it | MIT | [`LICENSE`](LICENSE) |
| Third-party licenses | Attribution the dependencies require | [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) |

Being named here is not a claim on anything, authorship, ownership or otherwise, and it is not an
affiliation: no one named here endorses Hertzforge or is responsible for it.

---

## Authors

Hertzforge is written by Brandon Zwink and Kevin Cross, who hold the copyright in it; see
[`LICENSE`](LICENSE).


## With Thanks To

<!-- Add entries here. A name and one line of what it was for; the specific
     thing is worth more than the adjective. Keep it in alphabetical order by
     surname so nobody has to read a ranking into the order. -->

*Nobody yet. This page was written before there was anyone on it, so that the policy would be
settled before the first case rather than decided about a particular person.*


## Organizations

<!-- The same, for groups. Naming a group rather than its individual members
     is often the better choice: some organizations have policies about staff
     being named in outside products, and a collective acknowledgment carries
     the thanks without asking anyone to clear it. -->

*Nothing yet.*


## Standing On

Not people, but worth naming: Hertzforge is written against a body of work it does not include
and could not replace. The dependencies it ships with, and what their licenses ask in return, are
in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md); this is the shorter list of what shaped it.
Papers are cited by author, title and venue. The control laws and the reference tools of MIMO
Random were written from these papers, and their code comments cite them the same way.

- **Bendat and Piersol**, *Random Data: Analysis and Measurement Procedures*. The spectral
  conventions follow it. Where the controller holds the complex conjugate of its cross-spectral
  definition, as the multiple-shaker control literature does, the documentation says so at every
  boundary.
- **Visual Dynamics**, Brandon Zwink's structural dynamics toolset, from which Hertzforge's
  analysis core comes. Its own acknowledgments carry over with it.
- The **UFF dataset 58** convention, for a fifty-year-old format that still says what a channel is
  and what it measures.
- **P. D. Welch**, "The Use of Fast Fourier Transform for the Estimation of Power Spectra", IEEE
  Transactions on Audio and Electroacoustics, 1967: the averaged spectral estimates every loop is
  built on.
- **IEC 60068-2-80**, *Environmental testing, Part 2-80: Tests, Test Fi: Vibration, Mixed mode*:
  the test type Mixed Mode controls, a random floor with sine tones or narrow bands of random laid
  over it. The control method is Hertzforge's own composition of published building blocks, each
  held to a closed-form case in its tests.
- **S. O. Rice**, "Mathematical Analysis of Random Noise", Bell System Technical Journal, 1944 and
  1945: the statistics of a sine wave in random noise, behind how far a tone must stand over the
  floor to be read and the predicted peak of the two together.
- **F. J. Harris**, "On the Use of Windows for Harmonic Analysis with the Discrete Fourier
  Transform", Proceedings of the IEEE, 1978: how far a tone leaks into the lines beside it, which
  sets the width Mixed Mode leaves out of the floor's average around each tone.
- **D. O. Smallwood**, the multiple-shaker random control papers: "Multiple Shaker Random Vibration
  Control: An Update", Proceedings of the IEST, 1999, the source of the default law's closed-loop
  correction of a factor of the reference and its per-loop limits; "Random Vibration Testing of
  a Single Test Item with a Multiple Input Control System", Proceedings of the IES, 1982, where the
  lineage starts; and "Multiple-Input Multiple-Output (MIMO) Linear Systems Extreme Inputs/Outputs",
  Shock and Vibration, 2007, with "Minimum Input Trace for Multiple Input Multiple Output Linear
  Systems", Journal of the IEST, 2013: the minimum drive principle, the exact-control end of the
  minimum-drive law.
- **K. Coletti, R. B. Davis and R. Schultz**, "Pareto-optimal target definition for multi-axis
  random vibration testing", 2026: the minimum-drive law's problem, the drive power traded against
  the autospectrum residuals over the positive semi-definite cone, and the automatic choice of the
  trade-off at the knee of the curve.
- **D. O. Smallwood and T. L. Paez**, "A Frequency Domain Method for the Generation of Partially
  Coherent Normal Stationary Time Domain Signals", Shock and Vibration, 1993: drive synthesis from
  a drive spectral density matrix.
- **R. Schultz and P. Avitabile**, "Shape-Constrained Input Estimation for Multi-Shaker Vibration
  Testing", IMAC proceedings, and "Shape-Constrained Input Estimation for Efficient Multi-shaker
  Vibration Testing", Experimental Techniques, 2020: the shape-constrained law.
- **R. Schultz**, "A Demonstration of Force Estimation and Regularization Methods for Multi-Shaker
  Testing", IMAC proceedings: the regularization choices and the condition-number limit of the
  identification gate.
- **P. M. Daborn, C. Roberts, D. J. Ewins and P. R. Ind**, "Next-Generation Random Vibration
  Tests", IMAC XXXII, 2014: the buzz-test method, matching the structure's own coherence and
  phase, as described by Schultz.
- **N. Hunter, K. Cross and G. Nelson**, "The Cross Spectrum in Multiple Input Multiple Response
  Vibration Testing", IMAC proceedings: which references a signal can have, entering one as
  autospectra, coherence and phase, and the eigenvalue clamp that repairs one that cannot be had.
- **L. Jacobs, M. Ross, G. Tipton, G. Nelson, K. Cross, N. Hunter and J. Harvie**, "6-DOF Shaker
  Test Input Derivation from Field Test" and "Experimental Execution of 6DOF Tests Derived from
  Field Tests", IMAC XXXV, 2017: six degree-of-freedom references from field data.
- **J. Paripovic and R. L. Mayes**, "Reproducing a Component Field Environment on a Six
  Degree-of-Freedom Shaker", IMAC proceedings: six degree-of-freedom control in practice,
  identification with uncorrelated inputs, and the conditioning a working test can tolerate.
- **N. J. Higham**, "Computing a Nearest Symmetric Positive Semidefinite Matrix", Linear Algebra and
  its Applications, 1988: the nearest realizable matrix, the mathematics under the reference
  repair.
- **Rattlesnake**, the open-source MIMO vibration controller by Sandia National Laboratories (its
  packaging names Daniel P. Rohe as author), for showing what an open MIMO controller should do.
  Credited as a source, not an affiliation. Nothing of its code is in Hertzforge: Rattlesnake is
  GPL-3.0 and Hertzforge is an MIT project, so its code could not have been taken. Its features,
  workflow and file layouts were read from its public documentation only, and the methods were
  written here from the published literature listed above.
- **sdynpy**, for showing what a structural dynamics toolset in Python should do. Hertzforge reads
  its data files; nothing of its code is in Hertzforge, since it is GPL-3.0 and its code could not
  have been taken into an MIT project.
