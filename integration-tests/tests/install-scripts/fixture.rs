// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    archive::{ArchiveFormat, Sha256},
    bare_tcp::{HangUpServer, StallServer},
    runner::report_problems,
    server::TestServer,
    shells::{PowerShellName, ShName, ShellKind},
    targets::Target,
    tls::TlsFront,
};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use color_eyre::eyre::{bail, eyre, Context, Result};
use httptest::{
    cycle,
    matchers::request,
    responders::{status_code, Responder},
    Expectation,
};
use mukti_metadata::{
    Digest, DigestAlgorithm, MuktiProject, MuktiReleasesJson, ReleaseLocation, ReleaseRangeData,
    ReleaseStatus, ReleaseVersionData, VersionRange,
};
use semver::Version;
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs,
    io::{self, Write as _},
    net::TcpStream,
    panic::{self, AssertUnwindSafe},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
};

const MUKTI_BIN: &str = env!("CARGO_BIN_EXE_mukti-bin-dup");
pub(crate) const HELPER: &str = env!("CARGO_BIN_EXE_mukti-test-helper");

// These are test values. The project and the binary have different names so
// that if the template mixes them up, the test fails.
pub(crate) const PROJECT: &str = "widget-project";
pub(crate) const BIN_NAME: &str = "widget";
pub(crate) const BIN_NAME_EXE: &str = "widget.exe";
const ENV_PREFIX: &str = "WIDGET_INSTALL";
pub(crate) const POST_INSTALL_ARGS: &[&str] = &["self", "setup"];
const SCRIPT_FLAGS: &[&str] = &[
    "--bin-name",
    BIN_NAME,
    "--base-url",
    "https://get.widget.example",
    "--docs-url",
    "https://widget.example/docs/install/",
    "--env-prefix",
    ENV_PREFIX,
    "--min-glibc",
    "2.27",
];
const POST_INSTALL_FLAGS: &[&str] = &[
    "--post-install-args",
    "self setup",
    "--post-install-min-version",
    "0.9.62",
];
pub(crate) const MANIFEST_FILE_NAME: &str = "install-manifest.txt";
const SHORT_LINE_SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The relative install directory, chosen to be problematic in a couple of ways
/// that would break naive scripts (leading hyphen, space in the directory
/// name).
const RELATIVE_INSTALL_DIR: &str = "-relative dir";

const HELPER_LOG_VAR: &str = "MUKTI_TEST_HELPER_LOG";
const HELPER_EXIT_VAR: &str = "MUKTI_TEST_HELPER_EXIT";
pub(crate) const OLD_BINARY: &[u8] = b"old binary\n";
pub(crate) const CALLER_INTACT: &str = "caller state intact";

/// Environment variables passed through to scripts.
///
/// All other environment variables are cleared so that things like the
/// developer's proxy settings and rc file cannot influence the test.
const PASSED_THROUGH_VARS: &[&str] = &[
    "PATH",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "ALLUSERSPROFILE",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_ARCHITEW6432",
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum PostInstallStep {
    Configured,
    #[cfg(not(windows))]
    Omitted,
}

impl PostInstallStep {
    pub(crate) fn script_flags(self) -> Vec<&'static str> {
        let post_install_flags = match self {
            Self::Configured => POST_INSTALL_FLAGS,
            #[cfg(not(windows))]
            Self::Omitted => &[],
        };
        SCRIPT_FLAGS
            .iter()
            .chain(post_install_flags)
            .copied()
            .collect()
    }

    /// Returns [`Self::script_flags`] with the value after `flag` replaced.
    pub(crate) fn script_flags_with<'a>(self, flag: &str, value: &'a str) -> Result<Vec<&'a str>> {
        let mut flags = self.script_flags();
        let position = flags
            .iter()
            .position(|existing| *existing == flag)
            .ok_or_else(|| eyre!("{flag} is not one of the script flags"))?;
        flags[position + 1] = value;
        Ok(flags)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Release {
    pub(crate) project: &'static str,
    pub(crate) version: &'static str,
    pub(crate) status: ReleaseStatus,
    pub(crate) targets: &'static [Target],
    pub(crate) checksum: ManifestChecksum,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ManifestChecksum {
    Correct,
    FirstDigitWrong,
    LastDigitWrong,
}

impl ManifestChecksum {
    fn apply(self, sha256: Sha256) -> Sha256 {
        match self {
            Self::Correct => sha256,
            Self::FirstDigitWrong => sha256.with_first_digit_changed(),
            Self::LastDigitWrong => sha256.with_last_digit_changed(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArchiveServing {
    Normal,
    /// A complete response holding only the first half of the archive. The
    /// checksum is expected to detect this scenario.
    Truncated,
    HangUp,
    /// The request is not responded to by the server, so the download hangs.
    Stall,
    UnavailableOnce,
    Redirect,
    NotFound,
}

/// Edits made to the generated manifests, for states the generator itself never
/// produces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManifestTweak {
    None,
    UnknownFormat,
    NotAManifest,
    CrlfLineEndings,
    NoFinalNewline,
    MalformedChecksum,
    PlainHttpArchiveUrl,
    /// An archive line cut off before the URL.
    ShortArchiveLine,
    MalformedVersion,
    UnknownStatus,
    UnknownKey,
    EmptyBody,
}

/// The ways in which install.sh can be started.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShMode {
    /// Run the script file.
    ///
    /// For example, `sh install.sh --version 0.9`.
    File,
    /// Run the script file with each option and its value as one argument separated by `=`.
    ///
    /// For example, `sh install.sh --version=0.9`.
    FileWithEquals,
    /// Simulate `curl | sh`.
    ///
    /// This allows for script parameters to be passed in via environment
    /// variables, but not as CLI arguments.
    Piped,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PowerShellMode {
    /// Run the script file with options, through the call operator, as someone
    /// who downloaded it would. For example:
    ///
    ///     powershell -Command "<trust setup>; & 'C:\...\install.ps1' -Version '0.9'"
    CallOperator,
    /// Run the script file with `powershell -File`. For example:
    ///
    ///     powershell -File C:\...\install.ps1 -Version 0.9
    ///
    /// In this scenario, there is no way to make PowerShell accept the test
    /// certificate.
    File,
    /// Simulate `irm | iex`.
    Piped,
    /// Fetch the script and invoke it as a script block with parameters.
    ScriptBlock,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Launch {
    Sh(ShMode),
    PowerShell(PowerShellMode),
}

/// A shell, and the way it starts the script.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShellLaunch {
    Sh(ShName, ShMode),
    PowerShell(PowerShellName, PowerShellMode),
}

impl ShellLaunch {
    pub(crate) fn launch(self) -> Launch {
        match self {
            Self::Sh(_, mode) => Launch::Sh(mode),
            Self::PowerShell(_, mode) => Launch::PowerShell(mode),
        }
    }
}

impl Launch {
    fn kind(self) -> ShellKind {
        match self {
            Self::Sh(_) => ShellKind::Sh,
            Self::PowerShell(_) => ShellKind::PowerShell,
        }
    }

    pub(crate) fn takes_options(self) -> bool {
        match self {
            Self::Sh(ShMode::File | ShMode::FileWithEquals)
            | Self::PowerShell(
                PowerShellMode::CallOperator | PowerShellMode::File | PowerShellMode::ScriptBlock,
            ) => true,
            Self::Sh(ShMode::Piped) | Self::PowerShell(PowerShellMode::Piped) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallDir {
    Plain,
    /// Square brackets in the name, which PowerShell treats as wildcards.
    WithBrackets,
    /// [`RELATIVE_INSTALL_DIR`].
    Relative,
    CargoHomeBin,
    HomeCargoBin,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HomeVars {
    CargoHomeAndHome,
    HomeOnly,
    #[cfg(not(windows))]
    Neither,
}

/// The default base URL embedded into a script.
///
/// If a test does not specify a base URL explicitly, the default is used.
#[derive(Clone, Copy, Debug)]
pub(crate) enum DefaultBaseUrl {
    /// The test server URL.
    TestServer,
    /// `get.widget.example`, which never resolves.
    Unreachable,
}

impl DefaultBaseUrl {
    /// Determines the appropriate default base URL to configure for a
    /// particular set of script options.
    ///
    /// If the script is passed a base URL explicitly, we configure the default
    /// to be unreachable to ensure that the script actually uses the provided
    /// base URL. Otherwise, we point it to the test server.
    pub(crate) fn for_options(options: ScriptOptions) -> Self {
        match options.base_url {
            Given {
                arg: None,
                env: None,
            } => Self::TestServer,
            Given { arg: Some(_), .. }
            | Given {
                arg: None,
                env: Some(_),
            } => Self::Unreachable,
        }
    }
}

/// A value given to a script as either an argument or an environment variable.
///
/// Values that depend on the fixture are resolved when the script runs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Value {
    Literal(&'static str),
    Target(Target),
    ServerUrl,
    ServerUrlWithSuffix(&'static str),
    ServerHostAndPort,
    ServerUrlAsPlainHttp,
    /// The install directory's full path.
    ///
    /// For [`InstallDir::Relative`], we pass in [`RELATIVE_INSTALL_DIR`]
    /// instead. In that scenario we expect the script to resolve against the
    /// directory it is run in.
    InstallDir(InstallDir),
}

/// How a script option is provided, via the environment or as an argument.
///
/// CLI arguments take precedence over environment variables.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Given {
    pub(crate) arg: Option<Value>,
    pub(crate) env: Option<Value>,
}

impl Given {
    pub(crate) const NONE: Self = Self {
        arg: None,
        env: None,
    };

    pub(crate) const fn arg(value: Value) -> Self {
        Self {
            arg: Some(value),
            env: None,
        }
    }

    pub(crate) const fn env(value: Value) -> Self {
        Self {
            arg: None,
            env: Some(value),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ScriptOption {
    sh_flag: &'static str,
    powershell_parameter: &'static str,
    env_suffix: &'static str,
}

impl ScriptOption {
    const fn new(
        sh_flag: &'static str,
        powershell_parameter: &'static str,
        env_suffix: &'static str,
    ) -> Self {
        Self {
            sh_flag,
            powershell_parameter,
            env_suffix,
        }
    }
}

const VERSION: ScriptOption = ScriptOption::new("--version", "-Version", "VERSION");
const BASE_URL: ScriptOption = ScriptOption::new("--base-url", "-BaseUrl", "BASE_URL");
const INSTALL_DIR: ScriptOption = ScriptOption::new("--install-dir", "-InstallDir", "DIR");
const TARGET: ScriptOption = ScriptOption::new("--target", "-Target", "TARGET");

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScriptOptions {
    pub(crate) version: Given,
    pub(crate) install_dir: Given,
    pub(crate) target: Given,
    pub(crate) base_url: Given,
}

impl ScriptOptions {
    pub(crate) const NONE: Self = Self {
        version: Given::NONE,
        install_dir: Given::NONE,
        target: Given::NONE,
        base_url: Given::NONE,
    };

    /// A [`ScriptOptions`] with the test server's URL and the plain install directory
    /// provided as arguments.
    pub(crate) const ARGS: Self = Self {
        install_dir: Given::arg(Value::InstallDir(InstallDir::Plain)),
        base_url: Given::arg(Value::ServerUrl),
        ..Self::NONE
    };

    /// A [`ScriptOptions`] with the test server's URL and the plain install directory
    /// provided as environment variables.
    pub(crate) const ENV: Self = Self {
        install_dir: Given::env(Value::InstallDir(InstallDir::Plain)),
        base_url: Given::env(Value::ServerUrl),
        ..Self::NONE
    };

    pub(crate) fn has_args(self) -> bool {
        self.by_option()
            .iter()
            .any(|(_, given)| given.arg.is_some())
    }

    fn by_option(self) -> [(ScriptOption, Given); 4] {
        [
            (BASE_URL, self.base_url),
            (VERSION, self.version),
            (INSTALL_DIR, self.install_dir),
            (TARGET, self.target),
        ]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Invocation<'a> {
    pub(crate) launch: ShellLaunch,
    pub(crate) options: ScriptOptions,
    pub(crate) home: HomeVars,
    /// Environment variables to set (Some) or remove (None) for the script.
    ///
    /// These are applied last.
    pub(crate) env: Vec<(&'a str, Option<String>)>,
    /// Replace `PATH` when set.
    pub(crate) path: Option<Utf8PathBuf>,
    pub(crate) post_install_exit_code: Option<u8>,
    /// For piped `sh` only -- the bytes to feed to the shell instead of the
    /// whole script, to simulate a truncated download.
    pub(crate) stdin: Option<Vec<u8>>,
    /// Extra arguments passed after the options.
    pub(crate) extra_args: &'a [&'a str],
}

impl Invocation<'_> {
    /// Defines a new invocation with default configuration.
    ///
    /// The default configuration is:
    ///
    /// * [`ScriptOptions::ARGS`]
    /// * [`HomeVars::CargoHomeAndHome`]
    /// * Everything else is empty or `None`.
    ///
    /// Callers can then override fields with struct update syntax.
    pub(crate) fn new(launch: ShellLaunch) -> Self {
        Self {
            launch,
            options: ScriptOptions::ARGS,
            home: HomeVars::CargoHomeAndHome,
            env: Vec::new(),
            path: None,
            post_install_exit_code: None,
            stdin: None,
            extra_args: &[],
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Outcome {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Outcome {
    pub(crate) fn from_output(output: Output) -> Self {
        Self {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "--- {} ---\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.status, self.stdout, self.stderr
        )
    }
}

/// The return value of a test body.
pub(crate) struct CheckedRun {
    /// A list of things that went wrong.
    ///
    /// Non-empty if nothing went wrong.
    pub(crate) problems: Vec<String>,
    /// The script's exit code and output.
    pub(crate) outcome: Outcome,
}

impl CheckedRun {
    pub(crate) fn report(self) -> Result<()> {
        let outcome = self.outcome;
        report_problems(self.problems).map_err(|error| eyre!("{error}\n{outcome}"))
    }
}

/// Returns the names of the entries in the directory.
pub(crate) fn dir_entry_names(dir: &Utf8Path) -> Result<BTreeSet<String>> {
    let entries = match dir.read_dir_utf8() {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error).wrap_err_with(|| format!("failed to read {dir}")),
    };
    entries
        .map(|entry| {
            let entry = entry.wrap_err_with(|| format!("failed to read {dir}"))?;
            Ok(entry.file_name().to_owned())
        })
        .collect()
}

fn placeholder_sha256(target: Target, format: ArchiveFormat) -> Sha256 {
    let placeholder = format!(
        "a placeholder for the {} archive for {target}\n",
        format.name()
    );
    Sha256::of(placeholder.as_bytes())
}

fn archive_url_path(target: Target, format: ArchiveFormat) -> String {
    format!("/archives/{target}.{}", format.name())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ServedArchive {
    pub(crate) target: Target,
    pub(crate) format: ArchiveFormat,
    pub(crate) serving: ArchiveServing,
    pub(crate) content: ArchiveContent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArchiveContent {
    /// The test helper binary.
    Helper,
    /// A file that cannot be started by the system.
    #[cfg(unix)]
    Unrunnable,
}

#[cfg(unix)]
const UNRUNNABLE_BINARY: &[u8] = b"#!/nonexistent/mukti-test-interpreter\n";

/// A loaded archive together with its contents, built once when the fixture is
/// set up.
struct LoadedArchive {
    served: ServedArchive,
    bytes: Vec<u8>,
    sha256: Sha256,
    /// The hash of the binary inside the archive.
    binary_sha256: Sha256,
}

impl LoadedArchive {
    fn load(served: ServedArchive) -> Result<Self> {
        let ServedArchive {
            target,
            format,
            content,
            serving: _,
        } = served;
        let member_name = target.bin_file_name();
        let binary = match content {
            ArchiveContent::Helper => fs::read(HELPER)
                .wrap_err_with(|| format!("failed to read the test helper at {HELPER}"))?,
            #[cfg(unix)]
            ArchiveContent::Unrunnable => UNRUNNABLE_BINARY.to_vec(),
        };
        let bytes = format.build(member_name, &binary)?;
        Ok(Self {
            served,
            sha256: Sha256::of(&bytes),
            bytes,
            binary_sha256: Sha256::of(&binary),
        })
    }
}

pub(crate) fn generate_scripts(
    project: &str,
    out_dir: &Utf8Path,
    flags: &[&str],
) -> Result<Output> {
    let releases_json = out_dir.with_extension("json");
    let mut releases = MuktiReleasesJson::default();
    releases.projects.insert(
        project.to_owned(),
        MuktiProject {
            latest: None,
            ranges: BTreeMap::new(),
        },
    );
    write_json(&releases_json, &releases)?;
    let mut args = vec![
        "--json",
        releases_json.as_str(),
        "generate-install-scripts",
        out_dir.as_str(),
    ];
    args.extend(flags);
    run_mukti(&args)
}

fn powershell_command_parameters(options: &[(ScriptOption, String)]) -> String {
    options
        .iter()
        .map(|(option, value)| format!("{} '{value}'", option.powershell_parameter))
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_json(path: &Utf8Path, releases: &MuktiReleasesJson) -> Result<()> {
    let json =
        serde_json::to_string_pretty(releases).wrap_err("failed to serialize the release JSON")?;
    fs::write(path, json).wrap_err_with(|| format!("failed to write {path}"))
}

fn run_mukti(args: &[&str]) -> Result<Output> {
    Command::new(MUKTI_BIN)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .wrap_err_with(|| format!("failed to run {MUKTI_BIN}"))
}

fn check_success(subcommand: &str, output: &Output) -> Result<()> {
    if !output.status.success() {
        bail!(
            "{subcommand} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Returns the locations that a script would put the test binary in by default.
pub(crate) fn real_default_binaries() -> Vec<Utf8PathBuf> {
    let mut bin_dirs = Vec::new();
    if let Ok(cargo_home) = env::var("CARGO_HOME") {
        bin_dirs.push(Utf8PathBuf::from(cargo_home).join("bin"));
    }
    for home_var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = env::var(home_var) {
            bin_dirs.push(Utf8PathBuf::from(home).join(".cargo").join("bin"));
        }
    }
    bin_dirs
        .iter()
        .flat_map(|dir| [dir.join(BIN_NAME), dir.join(BIN_NAME_EXE)])
        .collect()
}

enum ArchiveOrigin {
    /// Serve the archive from the same server the script and manifest are
    /// served from.
    Server,
    /// Serve the archive from a different server than the script and manifest
    /// are served from, with a redirect.
    ServerRedirect,
    /// Serve the archive from a server that hangs up.
    ///
    /// The hang up server has a TLS front of its own, since archive URLs must
    /// be https.
    HangUp {
        server: HangUpServer,
        front: TlsFront,
    },
    /// Serve the archive from a server that doesn't respond.
    ///
    /// Similar to [`Self::HangUp`], the server has a TLS front of its own,
    /// since archive URLs must be https.
    Stall {
        server: StallServer,
        front: TlsFront,
    },
}

pub(crate) struct Fixture {
    temp: Utf8TempDir,
    server: TestServer,
    tweak: ManifestTweak,
    served: Option<LoadedArchive>,
    archive_origin: ArchiveOrigin,
}

impl Fixture {
    pub(crate) fn new(
        release: &Release,
        tweak: ManifestTweak,
        served: Option<ServedArchive>,
        post_install: PostInstallStep,
        default_base_url: DefaultBaseUrl,
    ) -> Result<Self> {
        let served = served.map(LoadedArchive::load).transpose()?;
        let temp = camino_tempfile::Builder::new()
            .prefix("mukti-install-test-")
            .tempdir()
            .wrap_err("failed to create a temporary directory")?;
        let server = TestServer::start(temp.path())?;
        let archive_origin = match served.as_ref().map(|loaded| loaded.served.serving) {
            Some(ArchiveServing::HangUp) => {
                let hang_up = HangUpServer::start()?;
                let front = server.front_for(hang_up.addr())?;
                ArchiveOrigin::HangUp {
                    server: hang_up,
                    front,
                }
            }
            Some(ArchiveServing::Stall) => {
                let stall = StallServer::start()?;
                let front = server.front_for(stall.addr())?;
                ArchiveOrigin::Stall {
                    server: stall,
                    front,
                }
            }
            Some(ArchiveServing::Redirect) => ArchiveOrigin::ServerRedirect,
            Some(
                ArchiveServing::Normal
                | ArchiveServing::Truncated
                | ArchiveServing::UnavailableOnce
                | ArchiveServing::NotFound,
            )
            | None => ArchiveOrigin::Server,
        };
        let fixture = Self {
            temp,
            server,
            tweak,
            served,
            archive_origin,
        };

        let out_dir = fixture.out_dir();
        let releases_json = fixture.temp.path().join("releases.json");
        write_json(&releases_json, &fixture.releases(release)?)?;
        let output = run_mukti(&[
            "--json",
            releases_json.as_str(),
            "generate-install-manifests",
            out_dir.as_str(),
        ])?;
        check_success("generate-install-manifests", &output)?;
        let server_url = fixture.base_url();
        let flags = match default_base_url {
            DefaultBaseUrl::Unreachable => post_install.script_flags(),
            DefaultBaseUrl::TestServer => {
                post_install.script_flags_with("--base-url", &server_url)?
            }
        };
        let output = generate_scripts(PROJECT, &out_dir, &flags)?;
        check_success("generate-install-scripts", &output)?;

        // Create the simulated home directory.
        fs::create_dir_all(fixture.home()).wrap_err("failed to create the home directory")?;
        fs::create_dir_all(fixture.work_dir())
            .wrap_err("failed to create the directory to run scripts in")?;
        Ok(fixture)
    }

    pub(crate) fn without_archive(release: &Release, kind: ShellKind) -> Result<Self> {
        Self::new(
            release,
            ManifestTweak::None,
            None,
            kind.post_install_step(),
            DefaultBaseUrl::Unreachable,
        )
    }

    pub(crate) fn base_url(&self) -> String {
        self.server.url()
    }

    pub(crate) fn served_binary_sha256(&self) -> Option<Sha256> {
        self.served.as_ref().map(|loaded| loaded.binary_sha256)
    }

    fn archive_url(&self, target: Target, format: ArchiveFormat) -> String {
        let path = archive_url_path(target, format);
        match &self.archive_origin {
            ArchiveOrigin::Server => format!("{}{path}", self.base_url()),
            ArchiveOrigin::ServerRedirect => format!("{}/redirect{path}", self.base_url()),
            ArchiveOrigin::HangUp { front, .. } => format!("{}{path}", front.base_url()),
            ArchiveOrigin::Stall { front, .. } => format!("{}{path}", front.base_url()),
        }
    }

    fn releases(&self, release: &Release) -> Result<MuktiReleasesJson> {
        let mut locations = Vec::new();
        for &target in release.targets {
            for &format in target.archive_formats() {
                let real = self.served.as_ref().filter(|loaded| {
                    loaded.served.target == target && loaded.served.format == format
                });
                let sha256 = match real {
                    Some(loaded) => release.checksum.apply(loaded.sha256),
                    None => placeholder_sha256(target, format),
                };
                locations.push(ReleaseLocation {
                    target: target.as_str().to_owned(),
                    format: format.name().to_owned(),
                    url: self.archive_url(target, format),
                    checksums: BTreeMap::from([(
                        DigestAlgorithm::SHA256,
                        Digest(sha256.to_string()),
                    )]),
                });
            }
        }

        let version: Version = release
            .version
            .parse()
            .wrap_err_with(|| format!("invalid test release version {}", release.version))?;
        let version_data = ReleaseVersionData {
            release_url: format!("{}/release", self.base_url()),
            status: release.status,
            locations,
            metadata: serde_json::Value::Null,
        };
        let range_data = ReleaseRangeData {
            // This field only needs to be plausible, not necessarily correct.
            latest: version.clone(),
            is_prerelease: !version.pre.is_empty(),
            versions: BTreeMap::from([(version.clone(), version_data)]),
        };
        let project = MuktiProject {
            latest: None,
            ranges: BTreeMap::from([(VersionRange::from_version(&version), range_data)]),
        };
        let mut releases = MuktiReleasesJson::default();
        releases
            .projects
            .insert(release.project.to_owned(), project);
        Ok(releases)
    }

    fn tweaked_manifest(&self, path: &Utf8Path) -> Result<String> {
        let manifest =
            fs::read_to_string(path).wrap_err_with(|| format!("failed to read {path}"))?;
        let set_field = |key: &str, index: usize, value: &str| -> String {
            manifest
                .lines()
                .map(|line| {
                    let mut fields: Vec<_> = line.split(' ').collect();
                    if fields[0] == key {
                        fields[index] = value;
                    }
                    format!("{}\n", fields.join(" "))
                })
                .collect()
        };
        let tweaked: String = match self.tweak {
            ManifestTweak::None => return Ok(manifest),
            ManifestTweak::UnknownFormat => manifest.replacen(
                "mukti-install-manifest 1\n",
                "mukti-install-manifest 2\n",
                1,
            ),
            ManifestTweak::NotAManifest => {
                "<!DOCTYPE html>\n<html><body>Sign in to the network.</body></html>\n".to_owned()
            }
            ManifestTweak::CrlfLineEndings => manifest.replace('\n', "\r\n"),
            ManifestTweak::NoFinalNewline => manifest.trim_end().to_owned(),
            ManifestTweak::MalformedChecksum => set_field("archive", 3, "not-a-checksum"),
            ManifestTweak::PlainHttpArchiveUrl => {
                set_field("archive", 4, "http://archives.widget.example/widget")
            }
            ManifestTweak::ShortArchiveLine => {
                format!("{manifest}archive made-up-target tar.gz {SHORT_LINE_SHA256}\n")
            }
            ManifestTweak::MalformedVersion => set_field("version", 1, "0.9"),
            ManifestTweak::UnknownStatus => set_field("status", 1, "retired"),
            // Simulate the situation where a key is added by a later mukti
            // version which older scripts must skip. It goes directly after the
            // header, so that a script that completely stopped reading the
            // manifest at an unknown key (as opposed to merely skipping over
            // it) wouldn't work at all.
            ManifestTweak::UnknownKey => manifest.replacen('\n', "\nfuture-key some value\n", 1),
            ManifestTweak::EmptyBody => String::new(),
        };
        if tweaked == manifest {
            bail!("the manifest tweak {:?} did not change {path}", self.tweak);
        }
        Ok(tweaked)
    }

    pub(crate) fn expect_manifest(&self, spec: &str) -> Result<()> {
        let path = self.out_dir().join(spec).join(MANIFEST_FILE_NAME);
        let url_path = format!("/{spec}/{MANIFEST_FILE_NAME}");
        if path.is_file() {
            let body = self.tweaked_manifest(&path)?;
            self.expect_get(url_path, 1, status_code(200).body(body));
        } else if self.tweak == ManifestTweak::None {
            self.expect_get(url_path, 1, status_code(404));
        } else {
            bail!(
                "there is no manifest for {spec} to apply {:?} to",
                self.tweak
            );
        }
        Ok(())
    }

    pub(crate) fn expect_archive(&self) -> Result<()> {
        let loaded = self
            .served
            .as_ref()
            .ok_or_else(|| eyre!("this fixture was not set up to serve an archive"))?;
        let ServedArchive {
            target,
            format,
            serving,
            content: _,
        } = loaded.served;
        let bytes = loaded.bytes.clone();
        let path = archive_url_path(target, format);
        match serving {
            ArchiveServing::Normal => self.expect_get(path, 1, status_code(200).body(bytes)),
            ArchiveServing::Truncated => {
                let half = bytes[..bytes.len() / 2].to_vec();
                self.expect_get(path, 1, status_code(200).body(half));
            }
            // Unavailable once => two attempts.
            ArchiveServing::UnavailableOnce => self.expect_get(
                path,
                2,
                cycle![status_code(503), status_code(200).body(bytes)],
            ),
            ArchiveServing::Redirect => {
                let location = format!("{}{path}", self.base_url());
                self.expect_get(
                    format!("/redirect{path}"),
                    1,
                    status_code(302).insert_header("Location", location),
                );
                self.expect_get(path, 1, status_code(200).body(bytes));
            }
            ArchiveServing::NotFound => self.expect_get(path, 1, status_code(404)),
            // The hang-up server counts these requests.
            ArchiveServing::HangUp => {}
            // The test waits for the stalled request.
            ArchiveServing::Stall => {}
        }
        Ok(())
    }

    pub(crate) fn expect_powershell_script(&self) -> Result<()> {
        let path = self.script(ShellKind::PowerShell);
        let body = fs::read(&path).wrap_err_with(|| format!("failed to read {path}"))?;
        self.expect_get("/install.ps1".to_owned(), 1, status_code(200).body(body));
        Ok(())
    }

    /// Returns the base URL under which the redirect is served.
    pub(crate) fn expect_manifest_redirect_to_http(&self) -> Value {
        let prefix = "/to-http";
        let path = format!("/latest/{MANIFEST_FILE_NAME}");
        self.expect_get(
            format!("{prefix}{path}"),
            1,
            status_code(302).insert_header(
                "Location",
                format!("{}{path}", self.server.plain_http_url()),
            ),
        );
        Value::ServerUrlWithSuffix(prefix)
    }

    fn expect_get(&self, path: String, requests: usize, responder: impl Responder + 'static) {
        self.server.expect(
            Expectation::matching(request::method_path("GET", path))
                .times(requests)
                .respond_with(responder),
        );
    }

    pub(crate) fn verify_requests(&mut self, problems: &mut Vec<String>) {
        let verified = panic::catch_unwind(AssertUnwindSafe(|| self.server.verify_and_clear()));
        if let Err(panic) = verified {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("the HTTP server's expectations were not met");
            problems.push(format!("unexpected HTTP requests: {message}"));
        }
    }

    /// Verifies the result of an HTTP request, appending to the list of
    /// problems as appropriate.
    ///
    /// Call this with the result of a test's body once the script has run.
    pub(crate) fn verify_requests_with(
        &mut self,
        result: Result<CheckedRun>,
    ) -> Result<CheckedRun> {
        let mut request_problems = Vec::new();
        self.verify_requests(&mut request_problems);
        match result {
            Ok(mut run) => {
                run.problems.extend(request_problems);
                Ok(run)
            }
            Err(error) if request_problems.is_empty() => Err(error),
            Err(error) => {
                bail!(
                    "{error:?}\n--- HTTP requests ---\n{}",
                    request_problems.join("\n")
                );
            }
        }
    }

    /// Calls [`Self::verify_requests_with`], then reports the result as an
    /// error if the list of problems is not empty.
    pub(crate) fn finish(&mut self, result: Result<CheckedRun>) -> Result<()> {
        self.verify_requests_with(result)?.report()
    }

    pub(crate) fn take_hang_up_count(&self) -> Option<usize> {
        match &self.archive_origin {
            ArchiveOrigin::Server | ArchiveOrigin::ServerRedirect => None,
            ArchiveOrigin::HangUp { server, .. } => Some(server.take_hang_up_count()),
            ArchiveOrigin::Stall { .. } => None,
        }
    }

    pub(crate) fn wait_for_stalled_request(&self) -> Result<TcpStream> {
        match &self.archive_origin {
            ArchiveOrigin::Stall { server, .. } => server.wait_for_request(),
            ArchiveOrigin::Server
            | ArchiveOrigin::ServerRedirect
            | ArchiveOrigin::HangUp { .. } => {
                bail!("this fixture was not set up to stall the archive download");
            }
        }
    }

    fn out_dir(&self) -> Utf8PathBuf {
        self.temp.path().join("generated")
    }

    pub(crate) fn script(&self, kind: ShellKind) -> Utf8PathBuf {
        self.out_dir().join(match kind {
            ShellKind::Sh => "install.sh",
            ShellKind::PowerShell => "install.ps1",
        })
    }

    #[cfg(unix)]
    pub(crate) fn tool_dir(&self) -> Utf8PathBuf {
        self.temp.path().join("tools")
    }

    pub(crate) fn helper_log(&self) -> Utf8PathBuf {
        self.temp.path().join("helper-log.txt")
    }

    fn work_dir(&self) -> Utf8PathBuf {
        self.temp.path().join("work dir")
    }

    pub(crate) fn stray_work_dir_entries(
        &self,
        destination: Option<InstallDir>,
    ) -> Result<BTreeSet<String>> {
        let work_dir = self.work_dir();
        let mut names = dir_entry_names(&work_dir)?;
        if let Some(destination) = destination {
            let install_dir = self.install_dir(destination);
            names.retain(|name| work_dir.join(name) != install_dir);
        }
        Ok(names)
    }

    // `install.ps1` reads PowerShell's `$HOME`. On platforms other than
    // Windows, `pwsh` uses the value already set in `HOME`. On Windows, with
    // `HOME` and `USERPROFILE` set, Windows PowerShell 5.1 and PowerShell 7
    // report this directory.
    //
    // USERPROFILE is left unset on non-Windows platforms since that is a
    // Windows-only environment variable.
    fn set_home(&self, command: &mut Command) {
        command.env("HOME", self.home());
        #[cfg(windows)]
        command.env("USERPROFILE", self.home());
    }

    fn cargo_home(&self) -> Utf8PathBuf {
        self.temp.path().join("cargo home")
    }

    fn home(&self) -> Utf8PathBuf {
        self.temp.path().join("home dir")
    }

    pub(crate) fn install_dir(&self, dir: InstallDir) -> Utf8PathBuf {
        match dir {
            InstallDir::Plain => self.temp.path().join("install dir"),
            InstallDir::WithBrackets => self.temp.path().join("in[stall]"),
            InstallDir::Relative => self.work_dir().join(RELATIVE_INSTALL_DIR),
            InstallDir::CargoHomeBin => self.cargo_home().join("bin"),
            InstallDir::HomeCargoBin => self.home().join(".cargo").join("bin"),
        }
    }

    /// Returns the paths that must not exist after a run.
    ///
    /// This includes:
    ///
    /// * The default locations, other than the destination.
    /// * The real paths on the machine (we override environment variables to ensure this).
    pub(crate) fn forbidden_paths(&self, destination: Option<InstallDir>) -> Vec<Utf8PathBuf> {
        let cargo_home = self.cargo_home();
        let home_cargo = self.home().join(".cargo");
        let mut paths = match destination {
            Some(InstallDir::Plain | InstallDir::WithBrackets | InstallDir::Relative) | None => {
                vec![cargo_home, home_cargo]
            }
            Some(InstallDir::CargoHomeBin) => vec![home_cargo],
            Some(InstallDir::HomeCargoBin) => vec![cargo_home],
        };
        paths.extend(real_default_binaries());
        paths
    }

    pub(crate) fn resolve(&self, value: Value) -> Result<String> {
        let resolved = match value {
            Value::Literal(literal) => literal.to_owned(),
            Value::Target(target) => target.as_str().to_owned(),
            Value::ServerUrl => self.base_url(),
            Value::ServerUrlWithSuffix(suffix) => format!("{}{suffix}", self.base_url()),
            Value::ServerHostAndPort => self.server_host_and_port()?,
            Value::ServerUrlAsPlainHttp => format!("http://{}", self.server_host_and_port()?),
            Value::InstallDir(InstallDir::Relative) => RELATIVE_INSTALL_DIR.to_owned(),
            // Both PowerShell and Git Bash on Windows accept forward slashes.
            Value::InstallDir(
                dir @ (InstallDir::Plain
                | InstallDir::WithBrackets
                | InstallDir::CargoHomeBin
                | InstallDir::HomeCargoBin),
            ) => self.install_dir(dir).as_str().replace('\\', "/"),
        };
        Ok(resolved)
    }

    fn server_host_and_port(&self) -> Result<String> {
        let url = self.base_url();
        url.strip_prefix("https://")
            .map(str::to_owned)
            .ok_or_else(|| eyre!("the test server's URL, {url}, does not start with https://"))
    }

    pub(crate) fn run(
        &self,
        shell_path: &Utf8Path,
        invocation: &Invocation<'_>,
    ) -> Result<Outcome> {
        let (command, stdin) = self.command(shell_path, invocation)?;
        run_to_completion(command, stdin, shell_path)
    }

    pub(crate) fn spawn(
        &self,
        shell_path: &Utf8Path,
        invocation: &Invocation<'_>,
    ) -> Result<Child> {
        let (mut command, stdin) = self.command(shell_path, invocation)?;
        if stdin.is_some() {
            bail!("spawn does not support feeding the script on stdin");
        }
        // SAFETY: reset_signals_to_default is async-signal-safe.
        #[cfg(unix)]
        unsafe {
            command.pre_exec(reset_signals_to_default);
        }
        // Create a console for the subprocess, so a Ctrl-C we send there is
        // restricted to it.
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        #[cfg(windows)]
        restore_ctrl_c_handling()?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .wrap_err_with(|| format!("failed to run {shell_path}"))
    }

    fn command(
        &self,
        shell_path: &Utf8Path,
        invocation: &Invocation<'_>,
    ) -> Result<(Command, Option<Vec<u8>>)> {
        let launch = invocation.launch.launch();
        if invocation.options.has_args() && !launch.takes_options() {
            bail!(
                "{launch:?} runs the script without arguments, so options must be given \
                 through their environment variables"
            );
        }
        let mut options = Vec::new();
        let mut option_env = Vec::new();
        for (option, given) in invocation.options.by_option() {
            if let Some(value) = given.arg {
                options.push((option, self.resolve(value)?));
            }
            if let Some(value) = given.env {
                option_env.push((
                    format!("{ENV_PREFIX}_{}", option.env_suffix),
                    self.resolve(value)?,
                ));
            }
        }

        let mut command = Command::new(shell_path);

        // Scripts are run inside `self.work_dir`. We check that:
        //
        // * Relative install directories are resolved against it.
        // * The work dir is left clean at the end.
        //
        // Note that there's a PowerShell quirk -- `cd` changes PowerShell's
        // location but not the process's working directory. For example, in a
        // PowerShell started in `C:\Users\me`, after `cd D:\work`:
        //
        // * `-InstallDir bin` must mean `D:\work\bin`.
        // * But a .NET call such as `[IO.Directory]::CreateDirectory('bin')`
        //   would create `C:\Users\me\bin`.
        //
        // We need to tell the two apart in tests -- in order to do that, the
        // piped and script block launches start in the fixture root and then
        // set their location to the work dir.
        let work_dir = self.work_dir();
        command.current_dir(match launch {
            Launch::Sh(ShMode::File | ShMode::FileWithEquals | ShMode::Piped)
            | Launch::PowerShell(PowerShellMode::CallOperator | PowerShellMode::File) => {
                work_dir.clone()
            }
            Launch::PowerShell(PowerShellMode::Piped | PowerShellMode::ScriptBlock) => {
                self.temp.path().to_owned()
            }
        });

        let trust = match invocation.launch {
            ShellLaunch::Sh(shell, _) => {
                command.args(shell.args());
                String::new()
            }
            ShellLaunch::PowerShell(_, PowerShellMode::File) => String::new(),
            ShellLaunch::PowerShell(
                shell,
                PowerShellMode::CallOperator | PowerShellMode::Piped | PowerShellMode::ScriptBlock,
            ) => self.server.powershell_trust(shell),
        };
        let script = self.script(launch.kind());
        let script_url = format!("{}/install.ps1", self.base_url());
        let powershell = ["-NoProfile", "-ExecutionPolicy", "Bypass"];
        let mut stdin = None;
        let (takes_extra_args, takes_stdin) = match launch {
            Launch::Sh(ShMode::File | ShMode::FileWithEquals)
            | Launch::PowerShell(PowerShellMode::CallOperator | PowerShellMode::File) => {
                (true, false)
            }
            Launch::Sh(ShMode::Piped) => (false, true),
            Launch::PowerShell(PowerShellMode::Piped | PowerShellMode::ScriptBlock) => {
                (false, false)
            }
        };
        if !takes_extra_args && !invocation.extra_args.is_empty() {
            bail!("extra arguments are only supported for launches that run the script file with arguments");
        }
        if !takes_stdin && invocation.stdin.is_some() {
            bail!("replacing the script on stdin is only supported for piped sh");
        }
        match launch {
            Launch::Sh(ShMode::File) => {
                command.arg(script.as_str().replace('\\', "/"));
                for (option, value) in &options {
                    command.args([option.sh_flag, value]);
                }
                command.args(invocation.extra_args);
            }
            Launch::Sh(ShMode::FileWithEquals) => {
                command.arg(script.as_str().replace('\\', "/"));
                for (option, value) in &options {
                    command.arg(format!("{}={value}", option.sh_flag));
                }
                command.args(invocation.extra_args);
            }
            Launch::Sh(ShMode::Piped) => {
                stdin = Some(match &invocation.stdin {
                    Some(bytes) => bytes.clone(),
                    None => {
                        fs::read(&script).wrap_err_with(|| format!("failed to read {script}"))?
                    }
                });
            }
            Launch::PowerShell(PowerShellMode::CallOperator) => {
                let invoke = format!(
                    "{trust}& '{script}' {} {}",
                    powershell_command_parameters(&options),
                    invocation.extra_args.join(" ")
                );
                command.args(powershell).args(["-Command", &invoke]);
            }
            Launch::PowerShell(PowerShellMode::File) => {
                command.args(powershell).args(["-File", script.as_str()]);
                for (option, value) in &options {
                    command.args([option.powershell_parameter, value]);
                }
                command.args(invocation.extra_args);
            }
            Launch::PowerShell(PowerShellMode::Piped) => {
                // iex runs in the caller's scope, so give the caller some
                // variables with names matching those in the script. Ensure
                // that the script doesn't alter these variables.
                //
                // For the check for leftover functions, we only look in the
                // Function: drive (yes, this is a real thing!) The more obvious
                // thing to do would be to e.g. run `Get-Command Write-Info`,
                // but that would make Windows PowerShell search every installed
                // module for it, which was observed to take over a minute on
                // GitHub runners.)
                let caller = format!(
                    "{trust}Set-Location -LiteralPath '{work_dir}'; & {{ \
                     $Version = 'caller'; $InstallDir = 'caller'; $Target = 'caller'; $BaseUrl = 'caller'; \
                     irm '{script_url}' | iex; \
                     if (\"$Version $InstallDir $Target $BaseUrl\" -cne 'caller caller caller caller') {{ throw 'install.ps1 changed the variables of its caller' }}; \
                     if (\"$args\" -cne '-Version 9.9.9 stray') {{ throw 'install.ps1 changed the arguments of its caller' }}; \
                     if ((Test-Path Function:\\Write-Info) -or (Test-Path Function:\\Get-Url)) {{ throw 'install.ps1 left functions defined' }}; \
                     if (\"$ErrorActionPreference\" -cne 'Continue') {{ throw 'install.ps1 changed the error preference of its caller' }}; \
                     '{CALLER_INTACT}' \
                     }} -Version 9.9.9 stray"
                );
                command.args(powershell).args(["-Command", &caller]);
            }
            Launch::PowerShell(PowerShellMode::ScriptBlock) => {
                let invoke = format!(
                    "{trust}Set-Location -LiteralPath '{work_dir}'; \
                     & ([scriptblock]::Create((irm '{script_url}'))) {}",
                    powershell_command_parameters(&options)
                );
                command.args(powershell).args(["-Command", &invoke]);
            }
        }

        command.env_clear();
        for name in PASSED_THROUGH_VARS {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env("NO_COLOR", "1");
        self.server.set_trust_env(&mut command);
        for (name, value) in option_env {
            command.env(name, value);
        }
        match invocation.home {
            HomeVars::CargoHomeAndHome => {
                command.env("CARGO_HOME", self.cargo_home());
                self.set_home(&mut command);
            }
            HomeVars::HomeOnly => self.set_home(&mut command),
            #[cfg(not(windows))]
            HomeVars::Neither => {}
        }
        command.env(HELPER_LOG_VAR, self.helper_log());
        if let Some(code) = invocation.post_install_exit_code {
            command.env(HELPER_EXIT_VAR, code.to_string());
        }
        if let Some(path) = &invocation.path {
            command.env("PATH", path);
        }
        for (name, value) in &invocation.env {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }

        Ok((command, stdin))
    }
}

/// Resets `SIGHUP`, `SIGINT`, and `SIGTERM` to the default disposition.
///
/// This is run in the child between fork and exec, so it must be
/// async-signal-safe.
#[cfg(unix)]
fn reset_signals_to_default() -> io::Result<()> {
    // TODO-RAINCLAUDE: a shell cannot trap a signal it inherited as ignored,
    // as HUP is under nohup and INT in a background job, so the signal tests
    // depend on this.
    for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
        // SAFETY: signal is a valid signal number and SIG_DFL a valid signal
        // disposition. (We assume that libc is correct about the definitions
        // of these signals and SIG_DFL.)
        let previous = unsafe { libc::signal(signal, libc::SIG_DFL) };
        if previous == libc::SIG_ERR {
            // last_os_error() only reads errno. io::Error::new would allocate,
            // which is not async-signal-safe.
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Resets Ctrl-C handling to the default.
///
/// Some situations like Git Bash may start Cargo in a new process group, which
/// would disable Ctrl-C handling. We turn it off in this process so that `pwsh`
/// starts with normal handling.
///
/// Nextest is process-per-test, so only the current process is affected by
/// this.
#[cfg(windows)]
fn restore_ctrl_c_handling() -> Result<()> {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    // SAFETY: This is a correct call. (We assume that windows-sys declares the
    // function correctly.)
    if unsafe { SetConsoleCtrlHandler(None, 0) } == 0 {
        return Err(io::Error::last_os_error())
            .wrap_err("failed to restore normal Ctrl-C handling");
    }
    Ok(())
}

fn run_to_completion(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    shell_path: &Utf8Path,
) -> Result<Outcome> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command
        .spawn()
        .wrap_err_with(|| format!("failed to run {shell_path}"))?;
    // Write from another thread so that a shell that stops reading early
    // cannot block the test.
    let writer = stdin.map(|bytes| {
        let mut pipe = child.stdin.take().expect("stdin is piped");
        thread::spawn(move || match pipe.write_all(&bytes) {
            Ok(()) => Ok(()),
            // A broken pipe can occur if the shell exits early -- that is
            // probably deliberately constructed by the test and should not
            // be a test failure.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(error),
        })
    });
    let output = child
        .wait_with_output()
        .wrap_err_with(|| format!("failed to wait for {shell_path}"))?;
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| eyre!("the thread writing the script to the shell panicked"))?
            .wrap_err_with(|| format!("failed to write the script to {shell_path}'s stdin"))?;
    }

    Ok(Outcome::from_output(output))
}
