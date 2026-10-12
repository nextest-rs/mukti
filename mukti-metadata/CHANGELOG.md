# Changelog

## Unreleased

### Added

- `MuktiProject::latest_active_stable` and `ReleaseRangeData::latest_active_stable` return the newest version that is active and not a pre-release.

### Changed

- Parsing and deserializing a `VersionRange` now only accepts the form that `Display` writes. Components with leading zeros or a leading plus sign, such as `0.05` and `+1`, used to parse to the same range as `0.5` and `1`; they are now errors.
- **Breaking:** `VersionRangeParseError::error` is now a `VersionRangeComponentError` rather than a `ParseIntError`.

## [0.3.0] - 2024-11-24

### Added

Each `ReleaseLocation` now contains a `checksums` field for checksums of the release files.

## [0.2.1] - 2023-11-14

### Added

- `ReleaseVersionData` now contains a `metadata` field for custom metadata.

## [0.1.0] - 2022-06-12

Initial release.

[0.3.0]: https://github.com/nextest-rs/mukti/releases/tag/mukti-metadata-0.3.0
[0.2.1]: https://github.com/nextest-rs/mukti/releases/tag/mukti-metadata-0.2.1
[0.1.0]: https://github.com/nextest-rs/mukti/releases/tag/mukti-metadata-0.1.0
