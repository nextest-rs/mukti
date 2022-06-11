// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT or Apache-2.0

use crate::VersionRangeParseError;
use semver::Version;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize, Serializer};
use std::{collections::BTreeMap, fmt, str::FromStr};

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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseVersionData {
    /// Canonical URL for this release
    pub release_url: String,

    /// The status of a release
    pub status: ReleaseStatus,

    /// Release locations
    pub locations: Vec<ReleaseLocation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
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

#[derive(Clone, Copy, Debug)]
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
    s.parse()
        .map_err(|err| VersionRangeParseError::new(s, component, err))
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

impl<'de> Visitor<'de> for VersionRangeDeVisitor {
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
