// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::BTreeMap;

use blake2::Blake2b;
use bytes::Bytes;
use color_eyre::{eyre::eyre, Result};
use futures_util::stream::StreamExt;
use mukti_metadata::{Digest, DigestAlgorithm, MuktiReleasesJson, ReleaseLocation};
use sha2::{Digest as _, Sha256};
use tokio::task::JoinHandle;

use crate::command::Archive;

pub(crate) struct ArchiveWithChecksums {
    pub(crate) archive: Archive,
    pub(crate) url: String,
    // Err if fetching checksums failed.
    pub(crate) checksums: Result<Checksums>,
}

pub(crate) async fn fetch_release_checksums(
    archive_prefix: &str,
    archives: Vec<Archive>,
    download_jobs: usize,
) -> Vec<ArchiveWithChecksums> {
    let fetch_tasks = archives.iter().map(|archive| {
        let url = format!("{}/{}", archive_prefix, archive.name);
        async move {
            let result = spawn_fetch_and_checksum_task(url.clone()).await;
            (archive, url, result)
        }
    });

    // Note buffered rather than buffer_unordered, so results are obtained in
    // order.
    let mut stream = futures_util::stream::iter(fetch_tasks).buffered(download_jobs);

    let mut succeeded = 0;
    let mut failed = 0;

    let mut archives_with_checksums = Vec::new();

    while let Some((archive, url, result)) = stream.next().await {
        let checksums = match result {
            Ok(Ok(checksums)) => {
                succeeded += 1;
                Ok(checksums)
            }
            Ok(Err(e)) => {
                failed += 1;
                eprintln!("for {url}, error fetching checksum: {e}");
                Err(eyre!(e))
            }
            Err(e) => {
                failed += 1;
                eprintln!("for {url}, error waiting on checksum task: {e}");
                Err(eyre!(e))
            }
        };

        eprintln!(
            "fetched {}/{} checksums, {} failed",
            succeeded,
            archives.len(),
            failed
        );
        archives_with_checksums.push(ArchiveWithChecksums {
            archive: archive.clone(),
            url,
            checksums,
        });
    }

    archives_with_checksums
}

pub(crate) async fn backfill_checksums(release_json: &mut MuktiReleasesJson, download_jobs: usize) {
    let location_count = all_locations_without_checksums(release_json).count();

    let results = {
        let fetch_tasks = all_locations_without_checksums(release_json).map(|location| {
            // Note the spawn is inside the async block, which ensures that
            // the task is only spawned after being pulled off of the
            // buffer_unordered queue.
            let url = location.url.clone();
            async {
                let result = spawn_fetch_and_checksum_task(url.clone()).await;
                (url, result)
            }
        });

        let mut stream = futures_util::stream::iter(fetch_tasks).buffer_unordered(download_jobs);
        let mut results = BTreeMap::new();

        let mut succeeded = 0;
        let mut failed = 0;

        while let Some((url, result)) = stream.next().await {
            match result {
                Ok(Ok(checksum)) => {
                    // The checksum was fetched successfully -- update the release
                    // JSON with the new checksum.
                    results.insert(url, checksum);
                    succeeded += 1;
                }
                Ok(Err(e)) => {
                    eprintln!("for {url}, error fetching checksum: {e}");
                    failed += 1;
                }
                Err(e) => {
                    eprintln!("for {url}, error waiting on checksum task: {e}");
                }
            }

            eprintln!(
                "fetched {}/{} checksums, {} failed",
                succeeded, location_count, failed
            );
        }

        results
    };

    // Once all are done, update the release JSON with the new checksums.
    for project in release_json.projects.values_mut() {
        for range_data in project.ranges.values_mut() {
            for version in range_data.versions.values_mut() {
                for location in &mut version.locations {
                    if let Some(checksum) = results.get(&location.url) {
                        location.checksums = checksum.to_checksum_map();
                    }
                }
            }
        }
    }
}

fn all_locations_without_checksums(
    release_json: &MuktiReleasesJson,
) -> impl Iterator<Item = &ReleaseLocation> {
    all_locations(release_json).filter(|location| {
        !(location.checksums.contains_key(&DigestAlgorithm::SHA256)
            && location.checksums.contains_key(&DigestAlgorithm::BLAKE2B))
    })
}

fn all_locations(release_json: &MuktiReleasesJson) -> impl Iterator<Item = &ReleaseLocation> {
    release_json.projects.values().flat_map(|project| {
        project
            .all_versions()
            .flat_map(|(_, version_data)| &version_data.locations)
    })
}

fn spawn_fetch_and_checksum_task(url: String) -> JoinHandle<Result<Checksums, reqwest::Error>> {
    tokio::spawn(async move {
        // Attempt to fetch the URL 3 times.
        let bytes = {
            let mut attempt = 0;
            loop {
                match fetch_url(&url).await {
                    Ok(bytes) => break bytes,
                    Err(e) => {
                        eprintln!("Error fetching checksum: {}", e);
                        if attempt == 2 {
                            return Err(e);
                        }
                    }
                }
                attempt += 1;
            }
        };

        let sha256 = Sha256::digest(&bytes);
        let blake2b = Blake2b::digest(&bytes);

        Ok(Checksums {
            sha256: sha256.into(),
            blake2b: blake2b.into(),
        })
    })
}

async fn fetch_url(url: &str) -> reqwest::Result<Bytes> {
    let resp = reqwest::get(url).await?;
    resp.bytes().await
}

pub(crate) struct Checksums {
    sha256: [u8; 32],
    blake2b: [u8; 64],
}

impl Checksums {
    pub(crate) fn to_checksum_map(&self) -> BTreeMap<DigestAlgorithm, Digest> {
        [
            (DigestAlgorithm::SHA256, Digest(hex::encode(self.sha256))),
            (DigestAlgorithm::BLAKE2B, Digest(hex::encode(self.blake2b))),
        ]
        .into_iter()
        .collect()
    }
}
