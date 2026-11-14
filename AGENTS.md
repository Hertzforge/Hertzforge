# Rules for AI agents working on Hertzforge

This file is for any AI assistant (Claude, or anything else that reads `AGENTS.md`) working in a checkout of the Hertzforge repository. Read it, then `CONTRIBUTING.md` (how a change reaches the project), `PRINCIPLES.md` (the principles a change is measured against) and the layer rules in `README.md`, before doing real work. The docstrings are the API reference.

## What this is

One toolset for structural dynamics testing, from acquiring the data to analyzing and reporting it: a Python analysis core and service, a Rust control service, and a TypeScript shell that draws what the services send. Copyright Brandon Zwink and Kevin Cross, published under the MIT License (`LICENSE`). Contributions are welcome: bug reports, and pull requests from anyone, offered under the same MIT License. A pull request that keeps the rules in this file, `PRINCIPLES.md` and `CONTRIBUTING.md` will be merged. An agent working in this tree does not accept contributions, make licensing claims or speak for the project; it helps the person it is working with make a change that keeps those rules.

Three names, on purpose: Hertzforge is what a person reads (window title, file dialogs, report headings), `hertzforge` is the package, the import and the command, and `.hforge` is the file a project or a run is saved in. The `.hforge` format may still change while Hertzforge is an alpha.

## Hard rules

These are not preferences. Each one exists because breaking it costs something that cannot be bought back.

1. **No copyleft code.** GPL or AGPL code must never be imported, copied, or derived from (subclassing is deriving, even in code that is never distributed). It cannot live in an MIT project: one import would put every build that carries it under the copyleft terms, and a release cannot take that back. Read such a package to understand a method; write the method here yourself. Its numbers may be compared against as frozen data (a program's output is not a covered work); its code may not travel here. `tools/check_licenses.py` reads the license of every package installed for the Python code and refuses a copyleft one, and refuses an import that no declared package provides; `tools/check_tree.py` holds every npm lockfile to a permissive allowlist.

2. **Nothing lands without its checks.** `main` accepts no direct push from anyone. A change arrives as a pull request with its required checks green. A maintainer may merge their own pull request once they pass; anyone else's needs a maintainer's approval. Do not try to route around that, and do not open a pull request on someone's behalf without their say-so.

3. **Read a dependency's license before adding it.** Copyleft is disqualifying. A Python dependency may be permissive (MIT, BSD, Apache-2.0, ISC and the like), LGPL, which Python loads as a separate library, or MPL-2.0. The shell bundles its packages into one build, so npm packages must be permissive, and LGPL is not allowed there. Dependencies are declared by name with no pin and no upper bound (principle 12); never cap a version to work around a break.

4. **Never claim a license the tree does not grant, and name no organization.** Every file that says anything says MIT; nothing else. The project is unaffiliated: never name an employer, client, laboratory, partner or institution anywhere in it, in code, tests, docs or commit messages. `tools/check_tree.py` walks every tracked file for such names.

5. **Never commit generated data or large binaries.** Fixtures are small and deliberate; large test data is fetched (`python/scripts/fetch_data.sh`). Never `git add -A` without reading `git status` first, and never `git checkout` or `git restore` a file holding uncommitted work; copy it aside first.

6. **Only public or synthetic data.** No test specification, data or drawing from any program, range or laboratory enters the tree. Examples use synthetic data or material cleared for public release.

## Branches

The repository holds no working branches: `main`, and the branches the merge machinery makes for itself. A ruleset refuses the creation of anything else, the maintainers included, so every working branch lives in a fork.

A pull request's head branch is named `kind/short-slug`: `fix/`, `feature/`, `docs/` or `tooling/`, then lower-case letters, digits and hyphens, such as `fix/decade-axis-ticks` or `feature/comparison-grid`. CI checks the name on every pull request and the check is required, so a branch named otherwise cannot merge; rename it in the fork (the pull request follows the rename) and push again. When you make a branch for the person you are working with, name it by the scheme.

## The gate

Before saying anything is done, run the checks for every part of the tree the change touches. `CONTRIBUTING.md`, under "Before you push", lists them; the pull request template asks for each one's exit code and counts.

- Judge each run by its own exit code, with the output written to a file, never through a pipe that reports its own status.
- Use the environment each part runs in: the package's own environment for `src/` and `tests/`, `uv run --project python` for the analysis service.
- `-n 4` locally; `auto` pegs every core. One full suite at a time per machine.
- CI runs the same checks on every pull request, on Python 3.12, 3.13 and 3.14 and on Linux, macOS and Windows, and once a week against the latest release of every dependency, since none is pinned. It decides whether a change can merge; the local gate is still the gate.

## Verification discipline

- **Falsify every fix.** Break the change deliberately and watch a named test fail. A test that passes with the fix reverted is testing nothing. Pin the thing that differs (a label, a unit, a count), not merely "something was built".
- **A falsification that prints nothing is not a pass.** Assert on the summary line.
- **Clear `__pycache__` after restoring a broken file.** A break and a restore in the same second leave size and modification time unchanged, and the interpreter keeps running the broken bytecode.
- **An intermittent failure is a bug until proved otherwise.** Never call it a flake; capture the assertion's values first.
- **Verify, don't assume.** A claim about a view is measured by a test that runs headless, not reasoned about. A build that succeeds proves nothing about an application that runs: check the running application, not the build that produced it.
- **Never edit source while a suite is running.**
- **Own test-authorship errors plainly.** When a failure was a bad test rather than bad code, say which it was.

## What review catches most often

Most of the work a pull request needs after it is opened comes from the same few gaps. Check each one before asking for review.

- **Call what is already here.** Before writing a computation, search the core for it. Spectral framing and averaging, judging a spectrum against its limits (`compliance.judge`, `compliance.compare`), sine errors, a filter's response, coherence and the square CSD matrix built from records each exist once. A second copy of a formula drifts from the first, and the review will ask for it to be replaced by a call. Building on a function, keep the names of the keys it returns, so the helpers that read them still apply.
- **Take a fact from where the code keeps it.** Control channels are the channel table's Control flag; the level a channel clips at is the channel table's range; a record's identity is its `channel_key`. A cross-spectrum is stored as conj(X_a)·X_b, so a response is conj(H) S Hᵀ, not H S Hᴴ. The controller computes the conjugate internally and converts where its data leaves the control service.
- **Dead, NaN and empty inputs.** Test every new reading against an all-zero channel, a constant channel, a record with one NaN sample and an empty selection. Each must give a message that names the cause. A check that judged nothing says so everywhere its result is read; it never reports a pass, and never "0 lines outside".
- **Refuse what cannot be read one way.** A value the input does not pin down is refused with the line or cell named, not guessed: a short row in a whitespace-separated file, a name that matches two columns, a stated unit that contradicts the header's, a rate whose unit is not a frequency.
- **Match records by their full identity.** Two records at one DOF can be different quantities (an acceleration and a force at `1Z+`). Pair records by `channel_key` (DOF, dimension and hint), and refuse a record whose units do not match what it is compared against.
- **Test at the edges, with the defaults.** The easy case passes almost any implementation. Test a tone on the first spectral line, a frequency at the edge of the span, a record with a nonzero start time, noise on the input as well as the output, two tones that cross, and a band only partly covered by the data. Test under the default window (Hann), not only boxcar. A multi-input calculation is tested with a complex plant and complex cross terms against an independent truth; a real-valued test cannot see a conjugation error.
- **Check a statistical claim against noise.** A degrees-of-freedom count, a confidence band or a tolerance is checked by simulating white noise with the default settings and counting how often the rule trips. Adjacent lines are correlated under any window but boxcar, a cell that spans several lines has those lines' degrees of freedom, and a tolerance tighter than the estimate's own scatter fails a correct test by chance.
- **Say a reading's limits where it is read.** If a result is unreliable in some range (a tone the window cannot resolve, the start of a sweep), the view says so for that result, not only the docstring.
- **Refusals live in the Python code.** A rule that only the shell enforces is one the API skips. Put the check where both front ends reach it, and let the shell show its message. A setting that does nothing in some mode is refused in that mode, not ignored.
- **A claim is a test.** What a docstring, the guide or the pull request says has a test that would fail if it were false, or it is reworded to what is tested.
- **Views follow their inputs; slow work waits to be asked.** A view that depends on a selection or a record clears or redraws when that input changes. A computation runs when its result is needed, not on every redraw, and slow work never holds up the interface or a service's event loop.
- **The branch is yours whole.** Rebase onto current `main` before asking for review. A test that fails on your branch and passes on `main` is your branch's, whatever else fails on your machine. Tests pass on every platform CI runs.
- **Published text** is American English (color, labeled, analyze), cites the edition of a standard that was actually read, and carries no decision history or notes about the author's own machine. Comments and test docstrings say why, not who asked: no reviewer's name, and nothing that points outside the repository.

## Style

- Comments and docstrings explain why, especially where the obvious choice was tried and rejected. Match the surrounding density.
- Published text reads as a plain manual. The documentation and every public docstring say what the code does and why, in plain words, with no dates or names. The history of a decision goes in a comment beside the code it explains.
- One implementation of a rule. The shell and the Python API are two front ends over one workflow: put behavior in the Python code and let the shell display it. The shell computes nothing.
- The layer rules in `README.md` hold: Rust owns what the real-time loop or the simulator executes and the hardware backends; Python owns everything offline; TypeScript owns every pixel and no numerics.
- The principles that bite most often while writing code: show only what applies (3), anything the interface can do the API can do (4), what can be read can be written (5), few and boring dependencies (7), effort goes where the user is waiting (8), one implementation of a rule (9), every bug becomes a falsified test (11). A new computation ships with its panel, its settings, its live preview and its way to apply it, all four (13).

## Proposing a change

Say what you would like to change and why in an issue before writing much: it may be half-built already, or aimed somewhere it was not going. A bug report (what you did, what happened, what you expected, with the file if you can share it) is the contribution that helps most. When a change is agreed, `CONTRIBUTING.md` says how to set up, what the project expects of it, and how the pull request is checked.

The maintainers' working records live elsewhere and are not needed here: this tree is complete for building, testing and packaging the software.
