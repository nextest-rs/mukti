// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT or Apache-2.0

use crate::{
    GlibcVersionComponent, GlibcVersionParseError, GlibcVersionParseErrorKind,
    VersionRangeComponentError, VersionRangeParseError,
};
use byte_wrapper::{HexSlice, HexVec};
use semver::{Version, VersionReq};
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize, Serializer};
use std::{borrow::Cow, collections::BTreeMap, fmt, num::ParseIntError, str::FromStr};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MuktiReleasesJson {
    /// The projects that are part of this releases.json.
    pub projects: BTreeMap<String, MuktiProject>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MuktiProject {
    /// The latest version range (key in the releases field) without any pre-releases.
    pub latest: Option<VersionRange>,

    /// Map of version range (major or minor version) to release data about it
    #[serde(serialize_with = "serialize_reverse")]
    pub ranges: BTreeMap<VersionRange, ReleaseRangeData>,
}

impl MuktiProject {
    /// Return all version data for this release, ordered by most recent version first.
    ///
    /// Includes pre-release and yanked versions.
    pub fn all_versions(&self) -> impl Iterator<Item = (&Version, &ReleaseVersionData)> {
        self.ranges
            .values()
            .rev()
            .flat_map(|range| range.versions.iter().rev())
    }

    /// Retrieve data for this exact version if found.
    ///
    /// Can include yanked or pre-release versions.
    pub fn get_version_data(&self, version: &Version) -> Option<(&Version, &ReleaseVersionData)> {
        // Ignore build metadata since it isn't relevant.
        self.all_versions()
            .find(|&(v2, _)| eq_ignoring_build_metadata(version, v2))
    }

    /// Returns the newest active, non-prerelease version across all ranges, if
    /// any.
    pub fn latest_active_stable(&self) -> Option<(&VersionRange, &Version, &ReleaseVersionData)> {
        self.ranges.iter().rev().find_map(|(range, data)| {
            data.latest_active_stable()
                .map(|(version, version_data)| (range, version, version_data))
        })
    }

    /// Retrieve the latest version that matches this `VersionReq`.
    ///
    /// This will match the latest non-pre-release, non-yanked version.
    pub fn get_latest_matching(&self, req: &VersionReq) -> Option<(&Version, &ReleaseVersionData)> {
        self.all_versions().find(|&(version, version_data)| {
            version_data.status == ReleaseStatus::Active && req.matches(version)
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseRangeData {
    /// The latest version within this range (can be a prerelease)
    pub latest: Version,

    /// True if this version range only has prereleases.
    pub is_prerelease: bool,

    /// All known versions
    #[serde(serialize_with = "serialize_reverse")]
    pub versions: BTreeMap<Version, ReleaseVersionData>,
}

impl ReleaseRangeData {
    /// Returns the newest active, non-prerelease version in this range, if any.
    pub fn latest_active_stable(&self) -> Option<(&Version, &ReleaseVersionData)> {
        self.versions
            .iter()
            .rev()
            .find(|(version, data)| version.pre.is_empty() && data.status == ReleaseStatus::Active)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseVersionData {
    /// Canonical URL for this release
    pub release_url: String,

    /// The status of a release
    pub status: ReleaseStatus,

    /// Release locations
    pub locations: Vec<ReleaseLocation>,

    /// Custom domain-specific information stored about this release.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseStatus {
    /// This release is active.
    Active,

    /// This release was yanked.
    Yanked,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseLocation {
    /// The target string
    pub target: String,

    /// The archive format (e.g. ".tar.gz" or ".zip")
    pub format: String,

    /// The URL the target can be downloaded at
    pub url: String,

    /// The checksums for the target as a map of algorithm to checksum. This is
    /// left open-ended to allow for new checksum algorithms to be added in the
    /// future.
    #[serde(default)]
    pub checksums: BTreeMap<DigestAlgorithm, Digest>,

    /// The oldest glibc the binary runs on.
    ///
    /// Only set for `linux-gnu` targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_glibc: Option<GlibcVersion>,
}

pub fn is_linux_gnu_target(target: &str) -> bool {
    target.contains("-linux-gnu")
}

/// A glibc version, `MAJOR.MINOR`, for example `2.27`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct GlibcVersion {
    pub major: u32,
    pub minor: u32,
}

impl GlibcVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for GlibcVersion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for GlibcVersion {
    type Err = GlibcVersionParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let Some((major, minor)) = input.split_once('.') else {
            return Err(GlibcVersionParseError::new(
                input,
                GlibcVersionParseErrorKind::NoMinor,
            ));
        };
        let component = |value: &str, component: GlibcVersionComponent| {
            parse_canonical(value).map_err(|error| {
                GlibcVersionParseError::new(
                    input,
                    GlibcVersionParseErrorKind::Component {
                        component,
                        value: value.to_owned(),
                        error,
                    },
                )
            })
        };
        Ok(Self {
            major: component(major, GlibcVersionComponent::Major)?,
            minor: component(minor, GlibcVersionComponent::Minor)?,
        })
    }
}

impl Serialize for GlibcVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for GlibcVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(GlibcVersionDeVisitor)
    }
}

struct GlibcVersionDeVisitor;

impl Visitor<'_> for GlibcVersionDeVisitor {
    type Value = GlibcVersion;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a glibc version in the format major.minor")
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        s.parse().map_err(|err| E::custom(err))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct DigestAlgorithm(Cow<'static, str>);

impl DigestAlgorithm {
    /// The SHA-256 checksum algorithm.
    pub const SHA256: Self = Self(Cow::Borrowed("sha256"));

    /// The BLAKE2b-512 checksum algorithm.
    pub const BLAKE2B: Self = Self(Cow::Borrowed("blake2b"));

    pub const fn new_static(algorithm: &'static str) -> Self {
        Self(Cow::Borrowed(algorithm))
    }

    pub fn new(algorithm: String) -> Self {
        Self(Cow::Owned(algorithm))
    }
}

/// A digest, encoded as a hex string.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct Digest(pub Cow<'static, HexSlice>);

impl Digest {
    pub const fn new_static(bytes: &'static [u8]) -> Self {
        Self(Cow::Borrowed(HexSlice::new(bytes)))
    }

    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Cow::Owned(HexVec::new(bytes)))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&*self.0, f)
    }
}

// The Debug impl for Digest isn't derived because a derived version would print
// `HexSlice` or `HexVec` depending on the Cow variant, which is an internal
// detail that's definitely not worth exposing to users.
impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Digest").field(&self.to_string()).finish()
    }
}

fn serialize_reverse<S, K, V>(map: &BTreeMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    K: Serialize,
    V: Serialize,
{
    let mut serialize_map = serializer.serialize_map(Some(map.len()))?;
    for (k, v) in map.iter().rev() {
        serialize_map.serialize_entry(k, v)?;
    }
    serialize_map.end()
}

/// Represents a range of versions
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum VersionRange {
    Patch(u64),
    Minor(u64),
    Major(u64),
}

impl VersionRange {
    pub fn from_version(version: &Version) -> Self {
        if version.major >= 1 {
            VersionRange::Major(version.major)
        } else if version.minor >= 1 {
            VersionRange::Minor(version.minor)
        } else {
            VersionRange::Patch(version.patch)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum VersionRangeKind {
    /// Patch version.
    Patch,
    /// Minor version.
    Minor,
    /// Major version.
    Major,
}

impl VersionRangeKind {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Patch => "patch",
        }
    }
}

impl fmt::Display for VersionRange {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Major(major) => write!(f, "{}", major),
            Self::Minor(minor) => write!(f, "0.{}", minor),
            Self::Patch(patch) => write!(f, "0.0.{}", patch),
        }
    }
}

impl FromStr for VersionRange {
    type Err = VersionRangeParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if let Some(patch_str) = input.strip_prefix("0.0.") {
            parse_component(patch_str, VersionRangeKind::Patch).map(Self::Patch)
        } else if let Some(minor_str) = input.strip_prefix("0.") {
            parse_component(minor_str, VersionRangeKind::Minor).map(Self::Minor)
        } else {
            parse_component(input, VersionRangeKind::Major).map(Self::Major)
        }
    }
}

fn parse_component(s: &str, component: VersionRangeKind) -> Result<u64, VersionRangeParseError> {
    parse_canonical(s).map_err(|error| VersionRangeParseError::new(s, component, error))
}

fn parse_canonical<T>(s: &str) -> Result<T, VersionRangeComponentError>
where
    T: FromStr<Err = ParseIntError> + ToString + Into<u64> + Copy,
{
    let value: T = s.parse().map_err(VersionRangeComponentError::ParseInt)?;
    if value.to_string() != s {
        return Err(VersionRangeComponentError::NotCanonical {
            canonical: value.into(),
        });
    }
    Ok(value)
}

impl Serialize for VersionRange {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}", self))
    }
}

impl<'de> Deserialize<'de> for VersionRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(VersionRangeDeVisitor)
    }
}

struct VersionRangeDeVisitor;

impl Visitor<'_> for VersionRangeDeVisitor {
    type Value = VersionRange;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "a version range in the format major, major.minor, or major.minor.patch"
        )
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        s.parse().map_err(|err| E::custom(err))
    }
}

#[inline]
fn eq_ignoring_build_metadata(a: &Version, b: &Version) -> bool {
    a.major == b.major && a.minor == b.minor && a.patch == b.patch && a.pre == b.pre
}

#[cfg(test)]
mod tests {
    use super::*;

    static FIXTURE_JSON: &str = include_str!("../../fixtures/mukti-releases.json");

    #[test]
    fn test_get_version_data() {
        let json: MuktiReleasesJson = serde_json::from_str(FIXTURE_JSON).unwrap();
        let project = &json.projects["mukti"];
        // Active version matches
        assert_eq!(
            get_version_data(project, "0.5.3").release_url,
            "https://my-release-url/version-0.5.3",
            "data for active version 0.5.3 matches"
        );
        // Yanked version still matches.
        assert_eq!(
            get_version_data(project, "0.5.2").release_url,
            "https://my-release-url/version-0.5.2",
            "data for yanked version 0.5.2 matches"
        );
        // Pre-release version matches.
        assert_eq!(
            get_version_data(project, "0.6.0-alpha.1").release_url,
            "https://my-release-url/version-0.6.0-alpha.1",
            "data for pre-release 0.6.0-alpha.1 matches"
        );
    }

    #[track_caller]
    fn version(version_str: &str) -> Version {
        Version::parse(version_str)
            .unwrap_or_else(|err| panic!("unable to parse version string {version_str}: {err}"))
    }

    fn get_version_data<'a>(
        project: &'a MuktiProject,
        version_str: &str,
    ) -> &'a ReleaseVersionData {
        let version = version(version_str);
        let (_, version_data) = project
            .get_version_data(&version)
            .unwrap_or_else(|| panic!("no version data found for {version_str}"));
        version_data
    }

    #[test]
    fn test_get_latest_matching() {
        let json: MuktiReleasesJson = serde_json::from_str(FIXTURE_JSON).unwrap();
        let project = &json.projects["mukti"];

        // Latest version.
        assert_eq!(
            get_latest_matching_version(project, "*"),
            Some(&version("0.5.3")),
            "latest non-prerelease version"
        );
        assert_eq!(
            get_latest_matching_version(project, "0.5"),
            Some(&version("0.5.3")),
            "latest version in the 0.5 series"
        );

        // Version matching only pre-releases.
        assert_eq!(
            get_latest_matching_version(project, "0.6"),
            None,
            "0.6.0-alpha.1, being a pre-release, should not match 0.6.0"
        );

        // Version matching pre-release.
        assert_eq!(
            get_latest_matching_version(project, "0.6.0-alpha.1"),
            Some(&version("0.6.0-alpha.1")),
            "0.6.0-alpha.1 is a pre-release that matches this comparator"
        );

        // 0.5.2 is yanked.
        assert_eq!(
            get_latest_matching_version(project, "^0.5,<0.5.3"),
            Some(&version("0.5.1")),
            "0.5.2 is yanked so 0.5.1 should be returned"
        );
    }

    #[test]
    fn test_latest_active_stable() {
        let mut json: MuktiReleasesJson = serde_json::from_str(FIXTURE_JSON).unwrap();
        let project = json.projects.get_mut("mukti").unwrap();
        let range_0_5: VersionRange = "0.5".parse().unwrap();
        let range_0_6: VersionRange = "0.6".parse().unwrap();

        let latest = |project: &MuktiProject| {
            project
                .latest_active_stable()
                .map(|(range, version, _)| (*range, version.clone()))
        };
        assert_eq!(
            latest(project),
            Some((range_0_5, version("0.5.3"))),
            "0.6 only has a pre-release, so the latest stable version is in 0.5"
        );
        assert!(
            project.ranges[&range_0_6].latest_active_stable().is_none(),
            "a range with only pre-releases has no stable version"
        );

        let range = project.ranges.get_mut(&range_0_5).unwrap();
        range.versions.get_mut(&version("0.5.3")).unwrap().status = ReleaseStatus::Yanked;
        assert_eq!(
            latest(project),
            Some((range_0_5, version("0.5.1"))),
            "0.5.3 and 0.5.2 are yanked, so 0.5.1 is the latest stable version"
        );

        let range = project.ranges.get_mut(&range_0_5).unwrap();
        range.versions.get_mut(&version("0.5.1")).unwrap().status = ReleaseStatus::Yanked;
        assert_eq!(latest(project), None, "every stable version is yanked");
    }

    #[test]
    fn test_digest_serde() {
        let digest: Digest = serde_json::from_str(r#""0aFF""#).expect("mixed-case hex parses");
        assert_eq!(digest, Digest::new_static(&[0x0a, 0xff]));
        assert_eq!(digest.as_bytes(), &[0x0a, 0xff]);
        assert_eq!(digest.to_string(), "0aff");
        assert_eq!(
            serde_json::to_string(&digest).expect("digest serialized"),
            r#""0aff""#,
        );

        let owned = Digest::new(vec![0x0a, 0xff]);
        let borrowed = Digest::new_static(&[0x0a, 0xff]);
        assert_eq!(format!("{owned:?}"), r#"Digest("0aff")"#);
        assert_eq!(format!("{borrowed:?}"), r#"Digest("0aff")"#);

        for input in [r#""0g""#, r#""abc""#, r#""Not Found""#] {
            serde_json::from_str::<Digest>(input)
                .expect_err("a digest that isn't valid hex is rejected");
        }
    }

    #[test]
    fn test_version_range_parse_canonical() {
        let canonical = [
            ("0", VersionRange::Major(0)),
            ("3", VersionRange::Major(3)),
            ("18446744073709551615", VersionRange::Major(u64::MAX)),
            ("0.0", VersionRange::Minor(0)),
            ("0.5", VersionRange::Minor(5)),
            ("0.10", VersionRange::Minor(10)),
            ("0.0.0", VersionRange::Patch(0)),
            ("0.0.7", VersionRange::Patch(7)),
        ];
        for (input, expected) in canonical {
            let range: VersionRange = input
                .parse()
                .unwrap_or_else(|err| panic!("{input} should parse: {err}"));
            assert_eq!(range, expected, "{input} parses to the expected range");
            assert_eq!(range.to_string(), input, "{input} is displayed as written");
        }

        let not_canonical = [
            ("00", 0),
            ("03", 3),
            ("+3", 3),
            ("0.00", 0),
            ("0.05", 5),
            ("0.+5", 5),
            ("0.0.00", 0),
            ("0.0.07", 7),
            ("0.0.+7", 7),
        ];
        for (input, expected) in not_canonical {
            let err = input
                .parse::<VersionRange>()
                .expect_err("a non-canonical range is rejected");
            match err.error {
                VersionRangeComponentError::NotCanonical { canonical } => {
                    assert_eq!(canonical, expected, "canonical component for {input}");
                }
                VersionRangeComponentError::ParseInt(err) => {
                    panic!("{input} should be rejected as non-canonical, not with: {err}");
                }
            }
        }

        let not_integers = [
            "",
            "0.",
            "0.0.",
            "x",
            "1.2",
            "-1",
            " 1",
            "18446744073709551616",
        ];
        for input in not_integers {
            let err = input
                .parse::<VersionRange>()
                .expect_err("a range with a non-integer component is rejected");
            match err.error {
                VersionRangeComponentError::ParseInt(_) => {}
                VersionRangeComponentError::NotCanonical { canonical } => {
                    panic!("{input} should not parse as an integer, but parsed as {canonical}");
                }
            }
        }

        serde_json::from_str::<VersionRange>(r#""0.05""#)
            .expect_err("deserialization rejects a non-canonical range as well");
    }

    #[test]
    fn glibc_version_round_trips() {
        let version: GlibcVersion = "2.27".parse().expect("2.27 parsed");
        assert_eq!(version, GlibcVersion::new(2, 27));
        assert_eq!(version.to_string(), "2.27");
        assert_eq!(
            serde_json::to_string(&version).expect("serialized"),
            r#""2.27""#
        );
        assert_eq!(
            serde_json::from_str::<GlibcVersion>(r#""2.27""#).expect("deserialized"),
            version,
        );
        assert!(GlibcVersion::new(2, 27) < GlibcVersion::new(2, 28));
        assert!(GlibcVersion::new(2, 99) < GlibcVersion::new(3, 0));
    }

    #[test]
    fn glibc_version_errors_say_what_is_wrong() {
        let cases = [
            ("2", ""),
            ("02.27", ": major component `02` is invalid: not in canonical form (expected `2`)"),
            ("2.+27", ": minor component `+27` is invalid: not in canonical form (expected `27`)"),
            ("2.x", ": minor component `x` is invalid: invalid digit found in string"),
            ("2.", ": minor component `` is invalid: cannot parse integer from empty string"),
            ("2.27.1", ": minor component `27.1` is invalid: invalid digit found in string"),
            (
                "99999999999.1",
                ": major component `99999999999` is invalid: number too large to fit in target type",
            ),
        ];
        for (input, cause) in cases {
            let error = input
                .parse::<GlibcVersion>()
                .expect_err("invalid glibc version rejected");
            assert_eq!(
                error.to_string(),
                format!(
                    "invalid glibc version `{input}` (expected MAJOR.MINOR, for example 2.27){cause}"
                ),
            );
        }
        serde_json::from_str::<GlibcVersion>(r#""2.027""#)
            .expect_err("deserialization rejects a non-canonical version as well");
    }

    #[test]
    fn min_glibc_is_optional_in_json() {
        let json = r#"{"target": "x86_64-unknown-linux-musl", "format": "tar.gz", "url": "https://example.com/a.tar.gz"}"#;
        let location: ReleaseLocation = serde_json::from_str(json).expect("deserialized");
        assert_eq!(location.min_glibc, None);
        let serialized = serde_json::to_string(&location).expect("serialized");
        assert!(
            !serialized.contains("min_glibc"),
            "an unset floor is omitted: {serialized}"
        );

        let json = r#"{"target": "x86_64-unknown-linux-gnu", "format": "tar.gz", "url": "https://example.com/a.tar.gz", "min_glibc": "2.27"}"#;
        let location: ReleaseLocation = serde_json::from_str(json).expect("deserialized");
        assert_eq!(location.min_glibc, Some(GlibcVersion::new(2, 27)));
    }

    #[test]
    fn linux_gnu_targets() {
        for target in [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "riscv64gc-unknown-linux-gnu",
            "armv7-unknown-linux-gnueabihf",
        ] {
            assert!(is_linux_gnu_target(target), "{target}");
        }
        for target in [
            "x86_64-unknown-linux-musl",
            "x86_64-pc-windows-gnu",
            "x86_64-pc-windows-msvc",
            "universal-apple-darwin",
            "x86_64-unknown-illumos",
        ] {
            assert!(!is_linux_gnu_target(target), "{target}");
        }
    }

    fn get_latest_matching_version<'a>(
        project: &'a MuktiProject,
        version_req_str: &str,
    ) -> Option<&'a Version> {
        let version_req = VersionReq::parse(version_req_str).unwrap();
        let (version, _) = project.get_latest_matching(&version_req)?;
        Some(version)
    }
}
