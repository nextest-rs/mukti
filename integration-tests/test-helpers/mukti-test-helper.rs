// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A multicall binary for the install script tests.
//!
//! This acts as whatever program it is named after.
//!
//! * As uname, getconf, or isainfo it responds using `FAKE_*` variables.
//! * As a hash tool, it hashes stdin in the appropriate format.
//! * As curl, it records its arguments, then fails with an error on stderr.
//! * As anything else, it uses the `MUKTI_TEST_HELPER_LOG` and
//!   `MUKTI_TEST_HELPER_EXIT` environment variables.

use camino::Utf8Path;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, Read, Write as _},
    process::ExitCode,
};

fn main() -> ExitCode {
    let mut args = env::args();
    let program = args.next().unwrap_or_default();
    let name = Utf8Path::new(&program).file_stem().unwrap_or_default();
    let args: Vec<String> = args.collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match (name, args.as_slice()) {
        // OS configuration tools.
        ("uname", ["-s"]) => print_var("FAKE_UNAME_S"),
        ("uname", ["-m"]) => print_var("FAKE_UNAME_M"),
        ("uname", ["-o"]) => print_var("FAKE_UNAME_O"),
        ("getconf", ["GNU_LIBC_VERSION"]) => print_var("FAKE_GLIBC"),
        ("getconf", ["LONG_BIT"]) => print_var("FAKE_LONG_BIT"),
        ("isainfo", ["-n"]) => print_var("FAKE_ISAINFO"),

        // Hash tools. Each hash tool only accepts the arguments the install
        // script is meant to pass it.
        ("sha256sum", []) | ("shasum", ["-a", "256"]) => hash_stdin(|hash| format!("{hash}  -")),
        ("sha256", ["-q"]) | ("digest", ["-a", "sha256"]) => hash_stdin(|hash| hash.to_owned()),
        ("openssl", ["dgst", "-sha256"]) => hash_stdin(|hash| format!("SHA2-256(stdin)= {hash}")),
        (
            "uname" | "getconf" | "isainfo" | "sha256sum" | "shasum" | "sha256" | "digest"
            | "openssl",
            _,
        ) => fail(&format!("fake {name}: unexpected arguments {args:?}")),

        // Downloaders.
        ("curl", _) => {
            if let Err(error) = record(&args, "FAKE_CURL_LOG") {
                return fail(&error);
            }
            // This is curl's exit code for "could not connect".
            eprintln!("fake curl: (7) could not connect");
            ExitCode::from(7)
        }

        // Catch-all.
        _ => {
            if let Err(error) = record(&args, "MUKTI_TEST_HELPER_LOG") {
                return fail(&error);
            }
            match env::var("MUKTI_TEST_HELPER_EXIT") {
                Ok(code) => match code.parse::<u8>() {
                    Ok(code) => ExitCode::from(code),
                    Err(_) => fail(&format!(
                        "test helper: invalid MUKTI_TEST_HELPER_EXIT {code:?}"
                    )),
                },
                Err(_) => ExitCode::SUCCESS,
            }
        }
    }
}

fn print_var(variable: &str) -> ExitCode {
    match env::var(variable) {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(_) => fail(&format!("fake tool: {variable} is not set")),
    }
}

/// Appends the arguments, one per line, to the file named by the variable if it
/// is set.
fn record(args: &[&str], log_variable: &str) -> Result<(), String> {
    let Ok(log) = env::var(log_variable) else {
        return Ok(());
    };
    let mut contents = args.join("\n");
    contents.push('\n');
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&log)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .map_err(|error| format!("test helper: failed to write {log}: {error}"))
}

fn hash_stdin(format: impl FnOnce(&str) -> String) -> ExitCode {
    let mut input = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut input) {
        return fail(&format!("fake hash tool: failed to read stdin: {error}"));
    }
    println!("{}", format(&hex::encode(Sha256::digest(&input))));
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(1)
}
