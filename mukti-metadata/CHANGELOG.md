# Changelog

## [0.4.0] - 2026-11-13

### Added

- `MuktiProject::latest_active_stable` and `ReleaseRangeData::latest_active_stable` return the newest version that is active and not a pre-release.
- `ReleaseLocation` now has a `min_glibc` field: the oldest glibc that a `linux-gnu` binary runs on. It is serialized as a string (for example, `"min_glibc": "2.27"`), and omitted when unset.
- A new `GlibcVersion` type represents a glibc version in `MAJOR.MINOR` form. It parses from and displays as a string such as `2.27`; parse failures are reported as a `GlibcVersionParseError`.
- A new `is_linux_gnu_target` function returns whether a target triple is a `linux-gnu` target.

### Changed

- Parsing and deserializing a `VersionRange` now only accepts the form that `Display` writes. Components with leading zeros or a leading plus sign, such as `0.05` and `+1`, used to parse to the same range as `0.5` and `1`; they are now errors.
- **Breaking:** `VersionRangeParseError::error` is now a `VersionRangeComponentError` rather than a `ParseIntError`.
- **Breaking:** `Digest` is now a newtype over a `Cow<'static, HexSlice>` (from [byte-wrapper](https://docs.rs/byte-wrapper)) rather than a `String`. Digests are still written as lowercase hex strings, but deserializing a digest that isn't valid hex is now an error. As part of this change, equality comparisons now use the underlying bytes directly, ignoring hex digit case. Use `Digest::new` or `Digest::new_static` to build a digest from bytes, and `Digest::as_bytes` to read them back.
- **Breaking:** because of the new `min_glibc` field, code that builds a `ReleaseLocation` with a struct literal must now set it.
- The minimum supported Rust version is now 1.88.

## [0.3.0] - 2024-11-24

### Added

Each `ReleaseLocation` now contains a `checksums` field for checksums of the release files.

## [0.2.1] - 2023-11-14

### Added

- `ReleaseVersionData` now contains a `metadata` field for custom metadata.

## [0.1.0] - 2022-06-12

Initial release.

[0.4.0]: https://github.com/nextest-rs/mukti/releases/tag/mukti-metadata-0.4.0
[0.3.0]: https://github.com/nextest-rs/mukti/releases/tag/mukti-metadata-0.3.0
[0.2.1]: https://github.com/nextest-rs/mukti/releases/tag/mukti-metadata-0.2.1
[0.1.0]: https://github.com/nextest-rs/mukti/releases/tag/mukti-metadata-0.1.0
