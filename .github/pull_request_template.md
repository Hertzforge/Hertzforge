## What and why

One paragraph: what a user sees change, and the reason.

## Contracts touched

None | protocol | operation descriptors | view types | run files | units

(For any of these: schema, Python, Rust, TypeScript and golden frames are all in this pull request.)

## Test results

Each command as run, its exit code, and its pass / fail / skip counts:

```
uv run --project python pytest python -q -n 4    exit 0    N passed
npm --prefix ui test                              exit 0    N passed
python tools/check_tree.py                        exit 0    0 findings
```

## Falsified

Which named test fails when this change is broken on purpose, and what it reported.

## Mutation survivors (numeric modules only)

Each survivor, with one line: a gap now closed, or a change that cannot alter behavior.

## Views and figures

Every view or guide figure that changes, with its image, and the platforms it was looked at on.

## Hardware evidence (control changes only)

Chassis and modules, bench log, and the proof a declared capability rests on, or: "No hardware path touched."

## Names and licenses

The name and license checks pass. New dependencies, each with its license.
