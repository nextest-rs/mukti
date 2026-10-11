// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A multicall binary for the install script tests.
//!
//! This acts as whatever program it is named after.
//!
//! * As uname, getconf, or isainfo it responds using `FAKE_*` variables.
//! * As a hash tool, it hashes stdin in the appropriate format.
//! * As curl, it records its arguments, then fails with an error on stderr.
//! * On Windows, `mukti-test-helper send-ctrl-c <pid>` sends a Ctrl-C signal to the
//!   pid's console.
//! * As anything else, it uses the `MUKTI_TEST_HELPER_LOG` and
//!   `MUKTI_TEST_HELPER_EXIT` environment variables.

use byte_wrapper::HexArray;
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
        ("sha256sum", []) | ("shasum", ["-a", "256"]) => {
            hash_stdin(name, |hash| format!("{hash}  -"))
        }
        ("sha256", ["-q"]) | ("digest", ["-a", "sha256"]) => {
            hash_stdin(name, |hash| hash.to_owned())
        }
        ("openssl", ["dgst", "-sha256"]) => {
            hash_stdin(name, |hash| format!("SHA2-256(stdin)= {hash}"))
        }
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

        #[cfg(windows)]
        ("mukti-test-helper", ["send-ctrl-c", pid]) => send_ctrl_c(pid),

        // Keeps a copy of the binary running (and therefore, locked on Windows)
        // until the test closes stdin.
        (_, ["wait-for-stdin"]) => match io::stdin().read_to_end(&mut Vec::new()) {
            Ok(_) => ExitCode::SUCCESS,
            Err(error) => fail(&format!("test helper: failed to read stdin: {error}")),
        },

        // Catch-all.
        _ => {
            if let Err(error) = record(&args, "MUKTI_TEST_HELPER_LOG") {
                return fail(&error);
            }
            match env::var("MUKTI_TEST_HELPER_EXIT") {
                // Parse the exit code as an i32 so that Windows tests can exit
                // with an NTSTATUS such as 0xC0000135 (STATUS_DLL_NOT_FOUND;
                // present in the environment as -1073741515).
                Ok(code) => match code.parse::<i32>() {
                    Ok(code) => std::process::exit(code),
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

/// Records the tool's name in FAKE_HASH_LOG.
///
/// Tests can use this to tell that the fake tool was used.
fn hash_stdin(name: &str, format: impl FnOnce(&str) -> String) -> ExitCode {
    if let Err(error) = record(&[name], "FAKE_HASH_LOG") {
        return fail(&error);
    }
    let mut input = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut input) {
        return fail(&format!("fake hash tool: failed to read stdin: {error}"));
    }
    let digest = HexArray::<32>::new(Sha256::digest(&input).into());
    println!("{}", format(&digest.to_string()));
    ExitCode::SUCCESS
}

/// Sends a Ctrl-C signal to the console of the given process ID.
///
/// On Windows, Ctrl-C works per console. This function works by:
///
/// * Joining the console for the given process ID.
/// * Ignoring Ctrl-C on our own process.
/// * Sending the Ctrl-C signal to the console.
#[cfg(windows)]
fn send_ctrl_c(pid: &str) -> ExitCode {
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler, CTRL_C_EVENT,
    };

    let Ok(pid) = pid.parse::<u32>() else {
        return fail(&format!("send-ctrl-c: invalid process ID {pid:?}"));
    };
    // AttachConsole fails while we're on another console. FreeConsole's own
    // failure (meaning that we didn't already have a console) is fine to
    // ignore.
    //
    // SAFETY: no arguments; we assume that windows-sys has the correct Win32
    // declarations.
    unsafe { FreeConsole() };
    // SAFETY: an integer argument.
    if unsafe { AttachConsole(pid) } == 0 {
        return fail(&format!(
            "send-ctrl-c: failed to attach to the console of process {pid}: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: valid arguments.
    if unsafe { SetConsoleCtrlHandler(None, 1) } == 0 {
        return fail(&format!(
            "send-ctrl-c: failed to ignore Ctrl-C: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: valid arguments. Group 0 means the whole console.
    if unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) } == 0 {
        return fail(&format!(
            "send-ctrl-c: failed to send Ctrl-C to the console of process {pid}: {}",
            io::Error::last_os_error()
        ));
    }
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(1)
}
