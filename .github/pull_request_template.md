## What and why

One paragraph: what a user sees change, and the reason.

## Contracts touched

None | protocol | operation descriptors | view types | run files | units

(For any of these: schema, Python, Rust, TypeScript and golden frames are all in this pull request.)

## Test results

Each command as run, its exit code, and its pass / fail / skip counts:

```
python -m pytest tests -q -n auto --dist loadgroup -m "not slow and not timing"    exit 0    N passed, N skipped
python -m pytest tests -q -m timing               exit 0    N passed, N deselected
npm --prefix ui test                              exit 0    N passed
scripts/rust_tests.sh parallel                    exit 0    N passed
scripts/rust_tests.sh timing                      exit 0    N passed
cargo deny check                                  exit 0    advisories ok, bans ok, licenses ok, sources ok
python tools/check_tree.py                        exit 0    0 findings
```

## Falsified

Which named test fails when this change is broken on purpose, and what it reported.

## Mutation survivors (numeric modules only)

Each survivor, with one line: a gap now closed, or a change that cannot alter behavior.

## Views and figures

Every view or guide figure that changes, with its image, and the platforms it was looked at on.

## Drive path and hardware

<!--
Does this change a path listed under [drive path] in tools/drive_path.txt?
`python tools/check_drive_path.py --base origin/main` says, and CI checks this section
(docs/drive-path.md).

If not, keep the first line below and delete the four answers.
If it does, delete the first line and answer all four:
- What can now reach the drive: what this change can put on an output channel, or change about
  whether and how a run stops. "Nothing new" is an answer, if it is true.
- How it fails safe: what happens when it goes wrong (a bad value, a late tick, a dead thread),
  and the test that shows it.
- What ran on the simulator: the runs, and what they showed.
- What ran on hardware: the chassis and modules, the stated output limit, the bench log and the
  proof a declared capability rests on, or "Not on hardware" (it then goes on the bench list
  before a release).

A pull request that removes, ignores or weakens a test in tools/safety_tests.txt, or loosens a
limit, threshold, timeout or default on the drive path, carries "[weakens safety]" in its title
and says why here. Name any new dependency on the drive path that has unsafe code or a native
library.
-->

No drive-path code touched.

- What can now reach the drive:
- How it fails safe:
- What ran on the simulator:
- What ran on hardware:

## Names and licenses

The name and license checks pass. New dependencies, each with its license.
