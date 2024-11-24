// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::BTreeMap;

use blake2::Blake2b;
use bytes::Bytes;
use futures::stream::StreamExt;
use mukti_metadata::{Digest, DigestAlgorithm, MuktiReleasesJson, ReleaseLocation};
use sha2::{Digest as _, Sha256};
use tokio::task::JoinHandle;

pub(crate) async fn backfill_checksums(release_json: &mut MuktiReleasesJson, jobs: usize) {
    let location_count = all_locations(release_json).count();

    let results = {
        let fetch_tasks = all_locations(release_json)
            .filter(|location| {
                // Do all the checksums we want exist?
                !(location.checksums.contains_key(&DigestAlgorithm::SHA256)
                    && location.checksums.contains_key(&DigestAlgorithm::BLAKE2B))
            })
            .cloned()
            .map(|location| {
                // Note this is in an async block, which ensures that the task is only
                // spawned after being pulled off of the buffer_unordered queue.
                async {
                    let result = spawn_fetch_and_checksum_task(location.url.clone()).await;
                    (location.url, result)
                }
            });

        let mut stream = futures::stream::iter(fetch_tasks).buffer_unordered(jobs);
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
                    eprintln!("Error fetching checksum: {}", e);
                    failed += 1;
                }
                Err(e) => {
                    eprintln!("Error fetching checksum: {}", e);
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

fn all_locations(release_json: &MuktiReleasesJson) -> impl Iterator<Item = &ReleaseLocation> {
    release_json.projects.values().flat_map(|project| {
        project
            .all_versions()
            .flat_map(|(_, version_data)| &version_data.locations)
    })
}

fn spawn_fetch_and_checksum_task(url: String) -> JoinHandle<Result<Checksum, reqwest::Error>> {
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

        Ok(Checksum {
            sha256: sha256.into(),
            blake2b: blake2b.into(),
        })
    })
}

async fn fetch_url(url: &str) -> reqwest::Result<Bytes> {
    let resp = reqwest::get(url).await?;
    resp.bytes().await
}

struct Checksum {
    sha256: [u8; 32],
    blake2b: [u8; 64],
}

impl Checksum {
    fn to_checksum_map(&self) -> BTreeMap<DigestAlgorithm, Digest> {
        [
            (DigestAlgorithm::SHA256, Digest(hex::encode(self.sha256))),
            (DigestAlgorithm::BLAKE2B, Digest(hex::encode(self.blake2b))),
        ]
        .into_iter()
        .collect()
    }
}
