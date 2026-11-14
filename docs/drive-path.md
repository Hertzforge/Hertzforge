# The Drive Path

Hertzforge drives shakers. A shaker driven wrongly can break itself, the fixture or the article on
it, so one part of the code is held to rules the rest is not. This guide says which part, what those
rules are, and what a pull request that touches it must carry. It is written for anyone changing
the code, a person or a coding agent, who has not worked on the controller before.

Anyone may change anything, the drive path included. What protects a shaker is not who wrote the
change but the rules it meets, the checks that enforce them, and a run on a bench before a release.

## What the Drive Path Is

The drive path is code that can change what reaches an output channel, or whether and how a run
stops. That includes more than the code that writes a drive: the limits a drive is judged against,
the files those limits are read from, the loop's timing, the control laws, the control service and
its protocol, and the thread a run's data is written from.

`tools/drive_path.txt` lists it, one path per line with a reason, in two sections: `[drive path]`
and `[not drive path]`. Every crate under `crates/`, every folder under `modules/`, `laws/`, `wit/`
and `abi/`, and every entry directly under `schema/` is on exactly one of the two lists. A new
crate is unclassified until it is added, and CI fails until it is. To classify one, ask whether
it can put anything on an output channel, change a limit or a ceiling a drive is judged by, or
change when or how a run ends, directly or through a crate that does. If it can, it is drive
path.

Not on the drive path: the analysis service and the Python package (`src/hertzforge`), the shell
(`ui`), the readers and writers of recorded data, the recorder (it records inputs and opens no
output), and the analysis crates. The shell's Acquire & Control screens are a client of the
control service; by invariant 4 below, no protection may rest on them. The protocol files under
`schema/` hold both services' contract in one file each, so they are not listed: a change to the
control contract changes the Rust crates that carry it (`hertzforge-protocol`, `sds-control`) in
the same pull request, and those are drive path.

## The Invariants

These hold in every build, whoever changes what.

1. **The drive ceiling and the drive abort hold at all times, in every module.**
2. **A stop always ramps down; an abort always takes the drive to zero at once.**
3. **Any failure on a run thread ends in the emergency stop.** Panics are caught on the run
   thread, and the emergency stop is issued from there. `panic = "abort"` is not used while this is
   how drive safety works (`Cargo.toml` says why).
4. **Every protection lives in the control service.** The shell may show a limit or grey out a
   button, but it is never the only thing between an operator and the drive.
5. **Only the control service drives,** and only through the run commands of the control
   protocol, which pass its arm gates. The analysis service has no route to a drive.
6. **Anything read from a file that feeds a test is checked like a value typed in.** A reference,
   a test setup, a shaker record, a channel table or a control law is checked by the control
   service with the same checks as a value an operator enters, and the operator sees it before
   arming. Control law files run only in the sandboxed law host.
7. **The loop thread does no unbounded work.** File reading, the network and anything else that
   can block stay off it.

## The Safety Tests

`tools/safety_tests.txt` lists the tests that protect a shaker: the drive ceiling, the drive abort,
the stop ramp, the abort to zero, a run-thread failure, the law guards, the envelope, the kinematic
limits, divergence, overload, underrun, a dropped controller, a late tick, the input watchdog, the
arm gate, checked inputs, who may drive, and the shutdown. Each line names the crate, the file, the
test function and its kind, and says what it protects.

CI checks that every listed test exists in its file, is a test, is not ignored (`#[ignore]`, or a
`cfg_attr` that can add one) and is compiled into the test build: no `cfg` leaves it out on every
CI platform, and the file is reached from the crate root or is a test target the workspace builds.
The tests then run with every other test in the `control` job, or in the `timing` job when they
are listed in `scripts/rust_timing_tests.txt`.

A protection you add gets a test, and the test gets a line here. A protection with no test is a
gap to report, not something to list around.

## A Pull Request on the Drive Path

A pull request that changes a path on the drive-path list carries all of these.

| | Requirement | Checked by |
|---|---|---|
| D1 | A test that fails before the change and passes after, or the description says why that cannot be done and what was checked instead | Review |
| D2 | Every test in `tools/safety_tests.txt` still exists, is not ignored, and passes | CI |
| D3 | A safety test removed or weakened, or a limit, threshold, timeout or default on the drive path loosened, only when the title says `[weakens safety]` and the control maintainer approves | CI flags the list, review the rest |
| D4 | The template's "Drive path and hardware" section, filled in | CI |
| D5 | Review by the control maintainer, unless that maintainer wrote it | Review (code-owner review on these paths once it is configured) |
| D6 | No new dependency with unsafe code or a native library on the drive path without saying so in the description | Review, `cargo deny` |

### The Drive-Path Section

The pull request template's "Drive path and hardware" section has a line for a change that touches
none of it, `No drive-path code touched.`, and four questions for one that does:

- **What can now reach the drive:** what the change can put on an output channel, or change about
  whether and how a run stops. "Nothing new" is an answer when it is true.
- **How it fails safe:** what happens when it goes wrong, and the test that shows it.
- **What ran on the simulator:** the runs, and what they showed.
- **What ran on hardware:** the chassis and modules, the stated output limit, the bench log and the
  proof a declared capability rests on, or `Not on hardware`.

When the diff touches a drive-path path, CI fails until the line is gone and the four answers are
there. It judges the path against the drive-path list on both the base and the head, so a pull
request cannot take a path off the list and change it unannounced.

### Weakening a Protection

The title carries `[weakens safety]`, and the description says why, when a pull request:

- removes a line from `tools/safety_tests.txt`, which is also the only way to remove or ignore a
  listed test, since the check fails on a listed test that is gone or ignored;
- moves a path off the drive-path list;
- weakens a listed test (a looser bound, a smaller case, an assertion taken out), or loosens a
  limit, threshold, timeout or default on the drive path.

CI checks the first two. For the third it warns, naming each listed test whose body the diff edits,
and the reviewer judges. A test that moves to another file with its crate and name unchanged is
not a removal; a renamed test reads as one, so a rename either keeps the old name or carries the
title.

## Hardware

- Nobody drives a shaker from development code except under the bench rules: a stated output
  limit, a dummy load or a jumpered loop first, nothing downstream, and the run's evidence kept.
- A drive-path change is "not on hardware" until it has run on a bench. A release lists every
  drive-path change since the last one with its bench status, and does not ship one that has not
  run on a bench.
- A change that needs bench time says so in its pull request, and goes on the bench list the
  control maintainer keeps.

## Running the Check

```bash
python tools/check_drive_path.py                         # the two lists and the safety tests
python tools/check_drive_path.py --base origin/main \
    --title "the pull request's title" --body-file description.md
```

The second form also judges the branch as a pull request into `origin/main`: which drive-path
paths it touches, whether the description carries the section, and whether a removal needs the
title. Both exit 0 when clean, 1 with findings and 2 when the check could not run. The check needs
Python 3.11 or later and nothing outside the standard library; its own tests are in
`tools/tests/test_check_drive_path.py`.
