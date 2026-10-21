# General guidelines

This document captures code conventions for the mukti project.

## Paths

- **Always** use camino (`Utf8Path`, `Utf8PathBuf`) for paths.
- **Never** use `std::path::Path` or `std::path::PathBuf`, in tests as well as in production code.
- Use `camino-tempfile` for temporary files and directories, in tests as well as in production code.
- Prefer camino's own APIs (for example, `Utf8Path::read_dir_utf8`) where they exist. When an API hands back a std path, convert it at the boundary with `Utf8PathBuf::try_from` and return a real error if the conversion fails.

## Tests

- Run tests with `cargo nextest run`. mukti is part of the nextest project, so we dogfood nextest. Do not use `cargo test`, other than `cargo test --doc` for doctests.
