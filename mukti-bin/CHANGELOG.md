# Changelog

## [0.8.1] - 2026-11-16

### Added

- The install scripts written by `generate-install-scripts` now produce colored output ([#979]).

### Changed

- Generated install scripts now fail at startup if `CARGO_TERM_COLOR` is set to anything other than `auto`, `always`, or `never`, matching nextest and Cargo ([#979]).

### Security

- Update rustls to 0.23.46 for [GHSA-h2v8-394m-p7vv](https://github.com/advisories/GHSA-h2v8-394m-p7vv) and [GHSA-c8m5-c952-6cm4](https://github.com/advisories/GHSA-c8m5-c952-6cm4). mukti-bin does not enable rustls's Brotli certificate decompression, so it was not affected by GHSA-h2v8-394m-p7vv.

[#979]: https://github.com/nextest-rs/mukti/pull/979

[0.8.1]: https://github.com/nextest-rs/mukti/releases/tag/mukti-bin-0.8.1
