# Documentation and typing

Each of the three languages here has its own documentation conventions, and CI should check them the
same way in all three. The Python package is checked today. The Rust and TypeScript sections are a
proposal for the maintainers to agree on before anything is enforced, since turning a lint on
fails CI until every gap it finds is filled.

## Python (`src/hertzforge`), enforced

- Every public function, class and method has a docstring
  (`tests/test_docstrings.py::test_every_public_definition_has_a_docstring`).
- Every public function and method has annotated parameters and an annotated return
  (`tests/test_annotations.py::test_every_public_signature_is_typed`), and every public attribute is
  annotated where it is first assigned (`test_every_public_attribute_is_declared`).
- The scripting surface, meaning what `hertzforge.__all__` exports and the public methods of those
  classes, uses NumPy `Parameters` and `Returns` sections, and each parameter has its own entry.
  Everywhere else the docstring is prose: what the function does, and why when the reason is not
  obvious. These rules came over from Visual Dynamics with the code.

Public means the name has no leading underscore and its module path has none either (`__init__`
and `__main__` excepted). The checks read the source with `ast`, so they import nothing and take
about a second. Each check also has a test that plants the gaps it should find, so a check that
stops finding them fails too.

## Rust (the Cargo workspace), proposed

rustdoc is the standard: `//!` for a crate or module, `///` for an item, and the sections the Rust
API Guidelines name (C-FAILURE). A public function that returns `Result` has `# Errors`, one that
can panic has `# Panics`, and an `unsafe fn` has `# Safety`. Every crate already has a crate-level
`//!`. `hertzforge-protocol` and `hertzforge-backendd` already have `#![warn(missing_docs)]`.

The lints that check this, set once in the workspace `Cargo.toml`:

```toml
[workspace.lints.rust]
missing_docs = "warn"

[workspace.lints.clippy]
missing_errors_doc = "warn"
missing_panics_doc = "warn"
missing_safety_doc = "warn"

[workspace.lints.rustdoc]
broken_intra_doc_links = "warn"
```

The `control` job already runs clippy with `-D warnings`, so a warning there fails CI. A source
scan on 2026-11-01 found about 1,100 of the workspace's 5,837 public items and fields without a doc
comment, from 0 in `hertzforge-capture` to 149 in `hertzforge-sine`. That count is an upper bound,
since `missing_docs` skips items that are not reachable from outside the crate, and it does not
include enum variants. Turning the lints on for every crate at once would fail CI, so crates opt in
one at a time:

1. A crate opts in with `[lints] workspace = true` in its own `Cargo.toml` once it passes.
2. A test under `tools/tests/` holds the list of crates that have opted in and fails if one of them
   drops the line. Crates are only ever added to the list, the same way `ENFORCED` works in
   `tests/test_docstrings.py`. The first list is the crates that pass on the first CI run.
3. The `control` job adds `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`, which
   catches broken intra-doc links, something clippy does not check.

Types need no gate in Rust, since the compiler already requires them.

## TypeScript (the shell, `ui/src`), proposed

`tsconfig.json` already has `"strict": true`, and the `shell` job runs `svelte-check`, so the typing
half is already checked. For documentation, TSDoc is the standard: a `/** ... */` comment on every
export of `ui/src/lib`, and on the props of each Svelte component. A scan on 2026-11-01 found 95
of the 142 exports in `ui/src` without one, 51 of them in `lib/protocol/types.ts`.

The check should be a vitest test that walks the exports with the TypeScript compiler API, the way
the Python checks walk the AST. `typescript` is already a development dependency, so the test adds
nothing to `package.json`; `eslint-plugin-jsdoc` would do the same job but bring in ESLint and its
dependencies. The opt-in list works the same way as for the Rust crates, with one entry per
directory under `ui/src/lib`.
