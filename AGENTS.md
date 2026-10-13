# General guidelines

This document captures code conventions for the mukti project.

## Paths

- **Always** use camino (`Utf8Path`, `Utf8PathBuf`) for paths.
- **Never** use `std::path::Path` or `std::path::PathBuf`, in tests as well as in production code.
- Use `camino-tempfile` for temporary files and directories, in tests as well as in production code.
- Prefer camino's own APIs (for example, `Utf8Path::read_dir_utf8`) where they exist. When an API hands back a std path, convert it at the boundary with `Utf8PathBuf::try_from` and return a real error if the conversion fails.

## Tests

- Run tests with `cargo nextest run`. mukti is part of the nextest project, so we dogfood nextest. Do not use `cargo test`, other than `cargo test --doc` for doctests.
- Write tests in Rust. Do not add test drivers in Python or other scripting languages.
- The install scripts are tested end to end by `integration-tests/tests/install_scripts`, which runs the generated scripts under every shell it finds. Set `MUKTI_TEST_REQUIRE` to a comma-separated list of shells (for example, `dash,bash`) to make a missing one an error rather than an ignored test.
