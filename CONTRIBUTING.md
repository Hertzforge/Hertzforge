# Contributing

Thanks for taking a look.

Pull requests are welcome from anyone. Bug fixes, improvements to something that is already here, and new additions are all wanted, and you do not need an invitation or anyone's permission to open one. If something got in your way while you were using Hertzforge, the fix for it is a good first pull request.

A pull request that keeps the project's rules will be merged. The rules are written down, and every change is held to them, the maintainers' included: the principles (`PRINCIPLES.md`), the rules for working in the tree (`AGENTS.md`), the layer rules in `README.md`, and the two conditions below. A review checks a change against them. When a change needs more work, the review says which rule and why, and helps get it there. Leave *Allow edits by maintainers* ticked on the pull request, so that a small fix or a rebase onto `main` can be made on your branch instead of being sent back to you.

Small contributions count as much as large ones: a fix for one wrong number, a test for a case nobody covered, a clearer sentence in a guide, a reader for one more file format. A bug report is welcome too. Say what you did, what happened, and what you expected.

An issue before a pull request is never required. For a large addition, a short issue saying what you plan can still save you time, since it may already be half-built or there may be a simpler place for it. Questions are welcome as issues too.

If you work with an AI assistant, [`AGENTS.md`](AGENTS.md) is the same set of rules in the form an assistant reads first. Point the tool at it before it touches the tree.

Any contributed code has to meet two conditions:

- It has to be yours to give. It must be your own work, not something an employer already has a claim on, and it must carry no third-party code with obligations of its own. By opening a pull request you offer the change under the project's MIT License, the same terms everyone else receives it under.
- It may contain no code from any package under a copyleft license. GPL or AGPL code cannot be part of an MIT project: including it would make every build that contains it subject to the copyleft terms, which the project's license does not allow and which cannot be undone once a release is out. File formats and published methods may be implemented here from their descriptions; code may not be copied or derived from. There is no exception for tooling that is never distributed. `tools/check_licenses.py` reads the license of every package installed for the Python code and refuses a copyleft one, and refuses an import that no declared package provides.

## Licensing

Hertzforge is under the MIT License (`LICENSE`), copyright Brandon Zwink and Kevin Cross. You are free to install, use, copy, change and redistribute it, in open or closed work, as long as the copyright notice travels with it. Contributions come in under the same license they go out under, and there is no separate agreement to sign.

## Getting set up

```bash
git clone https://github.com/<you>/Hertzforge      # your fork
cd Hertzforge
pip install -e . pytest pytest-xdist jsonschema ruff   # the hertzforge package and its analysis service
scripts/fetch_testdata.sh                           # the plate test data the tests read
npm ci --prefix ui                                  # the shell
scripts/dev.sh                                      # all three services, shell at http://127.0.0.1:5191/
```

Python 3.12 or newer in an environment of its own, the current Node LTS, and the Rust toolchain named in `rust-toolchain.toml`. The control service and the controller crates are one Cargo workspace (`cargo test --workspace`); the NI backend loads the NI-DAQmx driver at run time, so nothing from NI is needed to build or test. `?mock=1` runs the shell alone, its control side on synthetic data. The desktop app is built with `desktop/packaging/build.sh` (`docs/desktop-app.md`).

## How a change travels

Work in a fork. The repository holds no working branches, only `main` and the branches the merge machinery makes for itself, and a ruleset refuses any other branch, the maintainers' included. Your fork is your own space: nobody else can create a branch there, and the fork's name says whose branch it is.

Name the branch for what it carries: `fix/`, `feature/`, `docs/` or `tooling/`, then a short lower-case name of letters, digits and hyphens, such as `fix/decade-axis-ticks` or `docs/random-workflow`. Open the pull request against `main`. A check on every pull request reads the branch name and fails until the name fits the scheme. Name it before you push: renaming the branch in your fork closes its pull request, and the work then goes up as a new one.

Nothing reaches `main` except through a pull request with its required checks green, and nobody pushes to `main` directly, the maintainers included. A maintainer may merge their own pull request once its checks pass. Anyone else's pull request needs a maintainer's approval as well.

## Before you push

```bash
ruff check .                                                       # lint, every Python file
python -m pytest tests -q -n 4 -m "not slow and not timing"        # the package and the analysis service
python -m pytest tools -q                                          # the tree checks' own tests
python tools/check_tree.py                                         # names and npm licenses
python tools/check_licenses.py --scope package                     # Python licenses and imports
python tools/check_drive_path.py --base origin/main --title "..." --body-file body.md
npm --prefix ui run check && npm --prefix ui test && npm --prefix ui run build
npm --prefix ui run build:report                                   # when the shell changed: commit the rebuilt viewer
scripts/rust_tests.sh parallel && cargo clippy --workspace --all-targets --locked -- -D warnings
cargo deny check                                                   # Rust licenses and advisories
```

Run the ones for the parts your change touches, and judge each by its own exit code. The plate test data (`scripts/fetch_testdata.sh`) has to be in place or about three hundred service tests fail. CI runs the same checks on every pull request, on every push to `main`, once a week, and on demand: the Python suites on 3.12, 3.13 and 3.14, the analysis service, the shell and the controller on Linux, macOS and Windows. The weekly run matters because no dependency is pinned (`PRINCIPLES.md`, 12).

The pull request template asks for each command as you ran it with its exit code and counts, the test that fails when the change is broken on purpose, and any new dependency with its license. Fill it in; a reviewer judges the change from it before running anything. Two of its sections are checked by CI. A change to a path in `tools/drive_path.txt` fills in "Drive path and hardware" with its four answers (`docs/drive-path.md`), and a change to a path in `tools/run_data_paths.txt` fills in all eight "Run data" fields (`docs/run-data.md`). CI reads the description as it stands, so after editing it, push again or rerun the check.

## What the project expects of a change

- One implementation of a rule. The shell and the Python API are two front ends over one workflow: put behavior in the Python code and let the shell display it. A rule written twice will eventually disagree with itself, and a formula written in two languages is the same mistake.
- Tests that would fail without the change. A test that checks the behavior is better than one that pins the implementation.
- Docstrings that say why. Write a comment when it states a constraint the code cannot show.
- No copyleft code, as set out in the second condition above.
- Every bug fix comes with a test that fails without the fix. Break the change on purpose and watch the test fail before calling it done; a test that passes either way is testing nothing.

Most of the fixes a review asks for fall into a few groups. [`AGENTS.md`](AGENTS.md), under "What review catches most often", has them in full. Briefly:

- Call the existing function for a computation the code already has (framing, judging against limits, coherence, a filter's response) instead of writing a second copy.
- Test each new reading against an all-zero channel, a NaN sample and an empty selection. Nothing judged is never a pass.
- Pair records by DOF, dimension and hint, and check their units.
- Test at the edges (the first line, the end of the span, a nonzero start time) and under the default Hann window. Check a statistical rule against simulated noise.
- Put refusals in the Python code, where the API and the shell both reach them.
- Take a fact from where the code keeps it: control channels from the channel table's Control flag, a CPSD's conjugation from the convention below.
- A claim in a docstring, the guide or the pull request has a test that would fail if it were false.
- Rebase onto current `main` before asking for review; a test that fails on your branch and passes on `main` is your branch's.

A cross-spectrum is stored as conj(X_a)·X_b, as Bendat and Piersol define it, everywhere in the core, in its files and in every message between the services. The conjugate is taken only where data crosses into or out of another convention: Rattlesnake and SDynPy files, and the MIMO control law interface. A multi-input calculation is tested with a complex plant, since a real-valued test cannot see a conjugation error.

Values are shown and saved in the display unit system the user chose, g included. A file format with no place for units holds those values as they are; a format that declares its units writes ones it can declare.

## Releasing

A maintainer releases with one command, `scripts/release.sh X.Y.Z notes.md`, the notes being what the release brings by pull request. It bumps the version in every file that states it (`scripts/bump_version.py`), merges the version pull request once its checks pass, signs and pushes the tag, watches the desktop build that publishes the apps, and checks that the release and the PyPI package are out. Run again after a stop, it picks up where it stopped.

## Reporting a problem

Say what you did, what happened, and what you expected. A `.hforge` project file or a small script that reproduces the problem is the fastest way to get it fixed.
