// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{collections::BTreeMap, fmt};

use blake2::Blake2b;
use bytes::Bytes;
use color_eyre::{
    eyre::{bail, eyre, Report, WrapErr},
    Result,
};
use futures_util::stream::StreamExt;
use mukti_metadata::{
    is_linux_gnu_target, Digest, DigestAlgorithm, GlibcVersion, MuktiReleasesJson, ReleaseLocation,
};
use sha2::{Digest as _, Sha256};
use tokio::task::JoinHandle;

use crate::{command::Archive, glibc::min_glibc_from_tar_gz};

pub(crate) struct FetchedArchive {
    pub(crate) archive: Archive,
    pub(crate) url: String,
    pub(crate) checksums: Checksums,
    pub(crate) min_glibc: Option<GlibcVersion>,
}

#[derive(Clone, Copy, Debug)]
enum GlibcFloor {
    NotApplicable,
    FromTarGz,
}

impl GlibcFloor {
    fn for_archive(target: &str, format: &str) -> Result<Self> {
        if !is_linux_gnu_target(target) {
            return Ok(Self::NotApplicable);
        }
        if format != "tar.gz" {
            bail!(
                "{target}.{format} is a glibc build, but mukti can only compute the glibc floor \
                 of a tar.gz archive"
            );
        }
        Ok(Self::FromTarGz)
    }
}

pub(crate) async fn fetch_release_archives(
    archive_prefix: &str,
    archives: Vec<Archive>,
    download_jobs: usize,
) -> Result<Vec<FetchedArchive>> {
    let floors = archives
        .iter()
        .map(|archive| {
            GlibcFloor::for_archive(&archive.target_format.target, &archive.target_format.format)
        })
        .collect::<Result<Vec<_>>>()
        .wrap_err("the release was not added")?;

    let fetch_tasks = archives.iter().zip(floors).map(|(archive, floor)| {
        let url = format!("{}/{}", archive_prefix, archive.name);
        async move {
            let result = fetch_archive(url.clone(), floor).await;
            (archive, url, result)
        }
    });

    // Note buffered rather than buffer_unordered, so results are obtained in
    // order.
    let mut stream = futures_util::stream::iter(fetch_tasks).buffered(download_jobs);

    let mut fetched = Vec::new();
    let mut download_failures = Vec::new();
    let mut floor_failures = Vec::new();
    let mut task_failures = Vec::new();

    while let Some((archive, url, result)) = stream.next().await {
        match result {
            Ok(info) => {
                fetched.push(FetchedArchive {
                    archive: archive.clone(),
                    url,
                    checksums: info.checksums,
                    min_glibc: info.min_glibc,
                });
            }
            Err(FetchError::Download(e)) => {
                eprintln!("for {url}, error downloading the archive: {e:#}");
                download_failures.push(format!("  {url}: {e:#}"));
            }
            Err(FetchError::GlibcFloor(e)) => {
                eprintln!("for {url}, error computing the glibc floor: {e:#}");
                floor_failures.push(format!("  {url}: {e:#}"));
            }
            Err(FetchError::Task(e)) => {
                eprintln!("for {url}, error in the fetch task: {e:#}");
                task_failures.push(format!("  {url}: {e:#}"));
            }
        }

        eprintln!(
            "fetched {}/{} archives, {} failed",
            fetched.len(),
            archives.len(),
            download_failures.len() + floor_failures.len() + task_failures.len(),
        );
    }

    let mut sections = Vec::new();
    if !download_failures.is_empty() {
        sections.push(format!(
            "failed to download {} archive(s):\n{}\n\
             check that --archive-prefix and the archive names are correct, \
             and that every archive has been uploaded",
            download_failures.len(),
            download_failures.join("\n"),
        ));
    }
    if !floor_failures.is_empty() {
        sections.push(format!(
            "downloaded {} linux-gnu archive(s), but could not compute their glibc floor:\n{}\n\
             the floor comes from the GLIBC_x.y symbol versions that the archive's ELF files \
             require; check that each archive holds a dynamically linked glibc build",
            floor_failures.len(),
            floor_failures.join("\n"),
        ));
    }
    if !task_failures.is_empty() {
        sections.push(format!(
            "{} fetch task(s) failed (this is a bug in mukti-bin):\n{}",
            task_failures.len(),
            task_failures.join("\n"),
        ));
    }
    if !sections.is_empty() {
        bail!(
            "{} of {} archive(s) failed, so the release was not added:\n{}",
            download_failures.len() + floor_failures.len() + task_failures.len(),
            archives.len(),
            sections.join("\n"),
        );
    }

    Ok(fetched)
}

#[derive(Debug, Default)]
pub(crate) struct BackfillOutcome {
    pub(crate) failures: Vec<BackfillFailure>,
}

#[derive(Debug)]
pub(crate) struct BackfillFailure {
    url: String,
    kind: BackfillFailureKind,
}

#[derive(Debug)]
enum BackfillFailureKind {
    Download(Report),
    GlibcFloor(Report),
    Task(Report),
    UnsupportedFormat(Report),
    ChecksumMismatch { computed: Digest, recorded: Digest },
}

impl BackfillFailure {
    fn new(url: &str, kind: BackfillFailureKind) -> Self {
        Self {
            url: url.to_owned(),
            kind,
        }
    }

    fn from_fetch(url: &str, error: FetchError) -> Self {
        let kind = match error {
            FetchError::Download(e) => BackfillFailureKind::Download(e),
            FetchError::GlibcFloor(e) => BackfillFailureKind::GlibcFloor(e),
            FetchError::Task(e) => BackfillFailureKind::Task(e),
        };
        Self::new(url, kind)
    }
}

impl fmt::Display for BackfillFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let url = &self.url;
        match &self.kind {
            BackfillFailureKind::Download(e) => write!(
                f,
                "{url}: failed to download: {e:#}\n    \
                 this may be temporary; run `mukti-bin backfill` again to retry"
            ),
            BackfillFailureKind::GlibcFloor(e) => write!(
                f,
                "{url}: downloaded, but could not compute the glibc floor: {e:#}\n    \
                 set min_glibc for this archive in the release JSON by hand"
            ),
            BackfillFailureKind::Task(e) => {
                write!(f, "{url}: {e:#}\n    this is a bug in mukti-bin")
            }
            BackfillFailureKind::UnsupportedFormat(e) => write!(
                f,
                "{url}: {e:#}\n    set min_glibc for this archive in the release JSON by hand"
            ),
            BackfillFailureKind::ChecksumMismatch { computed, recorded } => write!(
                f,
                "{url}: the downloaded archive has SHA-256 {computed}, but the release JSON \
                 records {recorded}; this location was not updated\n    \
                 find out why the archive changed; if the recorded checksums are wrong, \
                 remove them from the release JSON and run `mukti-bin backfill` again"
            ),
        }
    }
}

pub(crate) async fn backfill(
    release_json: &mut MuktiReleasesJson,
    download_jobs: usize,
) -> BackfillOutcome {
    let mut outcome = BackfillOutcome::default();

    // Keyed by URL, so that an archive listed more than once is only downloaded
    // once.
    let mut work: BTreeMap<String, GlibcFloor> = BTreeMap::new();
    for location in all_locations(release_json) {
        let needs_checksums = !has_all_checksums(location);
        let needs_floor = needs_glibc_floor(location);
        if !needs_checksums && !needs_floor {
            continue;
        }
        let floor = if needs_floor {
            match GlibcFloor::for_archive(&location.target, &location.format) {
                Ok(floor) => floor,
                Err(error) => {
                    outcome.failures.push(BackfillFailure::new(
                        &location.url,
                        BackfillFailureKind::UnsupportedFormat(error),
                    ));
                    continue;
                }
            }
        } else {
            GlibcFloor::NotApplicable
        };

        let entry = work.entry(location.url.clone()).or_insert(floor);
        match floor {
            GlibcFloor::FromTarGz => *entry = floor,
            GlibcFloor::NotApplicable => {}
        }
    }

    let total = work.len();
    let results = {
        let fetch_tasks = work.into_iter().map(|(url, floor)| {
            // Note the spawn is inside the async block, which ensures that
            // the task is only spawned after being pulled off of the
            // buffer_unordered queue.
            async move {
                let result = fetch_archive(url.clone(), floor).await;
                (url, result)
            }
        });

        let mut stream = futures_util::stream::iter(fetch_tasks).buffer_unordered(download_jobs);
        let mut results = BTreeMap::new();
        let mut failed = 0;

        while let Some((url, result)) = stream.next().await {
            match result {
                Ok(info) => {
                    results.insert(url, info);
                }
                Err(error) => {
                    let failure = BackfillFailure::from_fetch(&url, error);
                    eprintln!("error: {failure}");
                    outcome.failures.push(failure);
                    failed += 1;
                }
            }

            eprintln!(
                "fetched {}/{} archives, {} failed",
                results.len(),
                total,
                failed
            );
        }

        results
    };

    // Apply results after all downloads finished.
    for location in all_locations_mut(release_json) {
        let Some(info) = results.get(&location.url) else {
            continue;
        };
        let computed_sha256 = Digest::new(info.checksums.sha256.to_vec());
        if let Some(recorded) = location.checksums.get(&DigestAlgorithm::SHA256) {
            if *recorded != computed_sha256 {
                outcome.failures.push(BackfillFailure::new(
                    &location.url,
                    BackfillFailureKind::ChecksumMismatch {
                        computed: computed_sha256,
                        recorded: recorded.clone(),
                    },
                ));
                continue;
            }
        }
        if !has_all_checksums(location) {
            location.checksums = info.checksums.to_checksum_map();
        }
        if needs_glibc_floor(location) {
            location.min_glibc = info.min_glibc;
        }
    }

    // Downloads can finish in any order, so sort by URL to report failures in a
    // stable order.
    outcome
        .failures
        .sort_by(|left, right| left.url.cmp(&right.url));
    outcome
}

fn has_all_checksums(location: &ReleaseLocation) -> bool {
    location.checksums.contains_key(&DigestAlgorithm::SHA256)
        && location.checksums.contains_key(&DigestAlgorithm::BLAKE2B)
}

fn needs_glibc_floor(location: &ReleaseLocation) -> bool {
    is_linux_gnu_target(&location.target) && location.min_glibc.is_none()
}

fn all_locations(release_json: &MuktiReleasesJson) -> impl Iterator<Item = &ReleaseLocation> {
    release_json.projects.values().flat_map(|project| {
        project
            .all_versions()
            .flat_map(|(_, version_data)| &version_data.locations)
    })
}

fn all_locations_mut(
    release_json: &mut MuktiReleasesJson,
) -> impl Iterator<Item = &mut ReleaseLocation> {
    release_json.projects.values_mut().flat_map(|project| {
        project.ranges.values_mut().flat_map(|range_data| {
            range_data
                .versions
                .values_mut()
                .flat_map(|version_data| &mut version_data.locations)
        })
    })
}

struct ArchiveInfo {
    checksums: Checksums,
    min_glibc: Option<GlibcVersion>,
}

#[derive(Debug)]
pub(crate) enum FetchError {
    Download(Report),
    GlibcFloor(Report),
    Task(Report),
}

async fn fetch_archive(url: String, floor: GlibcFloor) -> Result<ArchiveInfo, FetchError> {
    match spawn_fetch_task(url, floor).await {
        Ok(result) => result,
        Err(error) => Err(FetchError::Task(eyre!(error))),
    }
}

fn spawn_fetch_task(url: String, floor: GlibcFloor) -> JoinHandle<Result<ArchiveInfo, FetchError>> {
    tokio::spawn(async move {
        // Attempt to fetch the URL 3 times.
        let bytes = {
            let mut attempt = 0;
            loop {
                match fetch_url(&url).await {
                    Ok(bytes) => break bytes,
                    Err(e) => {
                        eprintln!("Error fetching archive: {}", e);
                        if attempt == 2 {
                            return Err(FetchError::Download(eyre!(e.without_url())));
                        }
                    }
                }
                attempt += 1;
            }
        };

        let sha256 = Sha256::digest(&bytes);
        let blake2b = Blake2b::digest(&bytes);
        let checksums = Checksums {
            sha256: sha256.into(),
            blake2b: blake2b.into(),
        };

        let min_glibc = match floor {
            GlibcFloor::NotApplicable => None,
            GlibcFloor::FromTarGz => {
                let floor = tokio::task::spawn_blocking(move || min_glibc_from_tar_gz(&bytes))
                    .await
                    .map_err(|error| {
                        FetchError::Task(
                            eyre!(error).wrap_err("the glibc floor computation panicked"),
                        )
                    })?
                    .map_err(FetchError::GlibcFloor)?;
                Some(floor)
            }
        };

        Ok(ArchiveInfo {
            checksums,
            min_glibc,
        })
    })
}

async fn fetch_url(url: &str) -> reqwest::Result<Bytes> {
    // Without error_for_status, the body of a 404 (etc) page is checksummed as
    // if it were the archive.
    let resp = reqwest::get(url).await?.error_for_status()?;
    resp.bytes().await
}

pub(crate) struct Checksums {
    sha256: [u8; 32],
    blake2b: [u8; 64],
}

impl Checksums {
    pub(crate) fn to_checksum_map(&self) -> BTreeMap<DigestAlgorithm, Digest> {
        [
            (DigestAlgorithm::SHA256, Digest::new(self.sha256.to_vec())),
            (DigestAlgorithm::BLAKE2B, Digest::new(self.blake2b.to_vec())),
        ]
        .into_iter()
        .collect()
    }
}
