# Agent rules

These rules apply to all agents that change this repository. Obey all rules in all changes.

## Language: ASD-STE100 style

Write all new and changed text in a style like ASD-STE100 Simplified Technical English. This
includes doc comments, code comments, Markdown files and commit messages. Follow the writing
rules below. The STE dictionary does not apply: use any clear English word.

- Keep sentences short: about 20 words for an instruction, about 25 words for a description.
- Write one instruction in each sentence.
- Use the imperative form for instructions.
- Use the active voice.
- Prefer simple tenses.
- Give each word one meaning, and use the same term for the same item each time.
- Use the names from the code for code items (for example `ApiSetSchema`, `Resolution`).
- Use articles ("the", "a") where they make the text clearer.
- Avoid long noun clusters. Break up clusters of more than three words.
- Write short paragraphs with one topic each.
- Use a vertical list for a sequence of steps or for many items.
- Put a warning or a caution before the instruction it applies to.

Start the summary line of each doc comment with a verb in the imperative form (for example
"Return the host of the entry."). Do not use the third person ("Returns").

Write a commit message as `area: summary`, with a lowercase summary. Use the module name as the
area (for example `schema`, `resolve`, `builder`). For a change outside `src/`, use the
top-level item (for example `ci`, `fuzz`, `readme`).

## Lints: Clippy pedantic

The crate denies all warnings, all Clippy `pedantic` lints, `missing_docs` and `unsafe_code`.
The `[lints]` tables in `Cargo.toml`, `fuzz/Cargo.toml` and `tools/apiset-fetch/Cargo.toml` set
this.

- Keep the `[lints]` tables of the three manifests the same.
- Do not lower a lint level in a manifest or in the code.
- Do not use `#[allow(...)]`. The manifests deny `clippy::allow_attributes`.
- Use `#[expect(lint, reason = "...")]` only if no change to the code can remove the lint.
- Make sure that each expectation is fulfilled on 32-bit and 64-bit targets, with and without
  the `std` and `pe` features. An unfulfilled expectation is an error.
- Do not add an item to `clippy.toml` to hide a lint. Add a word to `doc-valid-idents` only for
  a product or architecture name in prose.
- `clippy.toml` sets `avoid-breaking-exported-api = false`. Fix a lint on the public API too.
- Do not add `unsafe` code.
- Make sure that these commands complete without warnings:

```sh
cargo clippy --all-features --all-targets -- -D warnings
cargo clippy --no-default-features --all-targets -- -D warnings
cargo clippy --no-default-features --features pe --all-targets -- -D warnings
cargo clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path tools/apiset-fetch/Cargo.toml --all-targets -- -D warnings
```

## File size: 1000 lines

- Keep each source file at a maximum of 1000 lines. This includes integration tests.
- If a change makes a file longer than 1000 lines, split the file into submodules.
- If you change a file that is already longer than 1000 lines, do not make it longer. Split it
  as part of the change.
- Put each submodule in a new file below the directory of its parent module.
- Split by topic. Do not split at a random line.

Clippy also limits each function to 100 lines (the `too_many_lines` default).

## Layout: Cargo package layout

Follow the Cargo package layout:
<https://doc.rust-lang.org/cargo/guide/project-layout.html>.

- Put the library root in `src/lib.rs`.
- Put integration tests in `tests/`.
- Put fuzz targets in `fuzz/fuzz_targets/`.
- Put development tools in `tools/<name>/`, each as its own package with `publish = false`.
- Put examples in `examples/` and benchmarks in `benches/`.
- For a test, example or benchmark with many files, make a directory with a `main.rs` file and
  the modules.
- Use snake_case names for modules.
- Put a module in `foo.rs` and its child modules in `foo/`. Do not use `mod.rs` files. The
  manifests deny `clippy::mod_module_files`.
- Re-export a new public type from the crate root (`src/lib.rs`).

## Library rules

- Keep the library usable as `no_std + alloc`. Import `alloc` items through
  `use crate::prelude::*;`. Put each item that needs `std` behind `#[cfg(feature = "std")]`.
- Put each item that needs `portex` behind `#[cfg(feature = "pe")]`.
- Treat all schema bytes as untrusted input. Do not panic on schema data.
- Use checked arithmetic and `TryFrom` for values from schema data. On failure, return an
  `Error` with the `ErrorKind` that matches the problem. Do not use an `as` cast that can
  truncate or wrap.
- Give each panicking function a `try_*` function that returns `Result`. Document the panic in
  a `# Panics` section, and refer to the `try_*` function.
- Support all schema versions: 2, 3, 4 and 6.
- Remember that `usize` has 32 bits on `i686-pc-windows-msvc`. CI checks this target.
- Resolve names as the Windows loader does. Keep the binary searches of the loader, also on
  data that is not sorted. Name the loader function in the docs (for example
  `ApiSetResolveToHost`).
- Read a versioned structure through its own `Size` field, as the Windows loader does.
- Keep round trips lossless at the model level: `ApiSetSchema::parse` of the output of
  `ApiSetSchemaBuilder::try_build` gives the same entries.
- In docs for a spec structure, name the Windows structure (for example
  `API_SET_NAMESPACE_ENTRY_V6`). For an offset field, say what it is relative to.
- Do not use language features or library APIs newer than Rust 1.88 (`rust-version`).
- A breaking change to the public API needs a new minor version while the version is `0.x`.
  List each breaking change in the pull request.
- The Rust examples in `README.md` compile as doctests. Keep them correct when the API changes.

## Tests

- Add a regression test for each defect. Make sure that the test fails before the fix.
- Build test schemas in the test, with `ApiSetSchemaBuilder` or explicit bytes. Do not read
  `apisetschema.dll` or other files from the host system.
- Make sure that the tests pass with and without the `std` and `pe` features.
- Do not use wall-clock time or random data without a fixed seed.

## Working copies

- Use one git worktree for each branch when agents work in parallel.
- Use absolute paths for all edits in a worktree. Do not edit the primary checkout from a
  worktree task.

## Checks

Make sure that all these checks pass before you report a change as complete:

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check
cargo fmt --manifest-path tools/apiset-fetch/Cargo.toml -- --check
cargo clippy --all-features --all-targets -- -D warnings
cargo clippy --no-default-features --all-targets -- -D warnings
cargo clippy --no-default-features --features pe --all-targets -- -D warnings
cargo clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path tools/apiset-fetch/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tools/apiset-fetch/Cargo.toml
cargo test --all-features
cargo test --no-default-features
cargo doc --no-deps --all-features
cargo +1.88.0 check --all-features --all-targets
```

CI also runs Clippy and the tests for `i686-pc-windows-msvc`. To check 32-bit `usize`, add
`--target i686-pc-windows-msvc` to the Clippy and test commands.

`mise run precommit` runs the format and Clippy checks.
