// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(unix)]
use crate::hang_up::StallServer;
use crate::{
    archive::{ArchiveFormat, Sha256},
    hang_up::HangUpServer,
    server::TestServer,
    shells::{ShellKind, ShellSpec},
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
use std::{
    collections::BTreeMap,
    env, fmt, fs,
    io::{self, Write as _},
    panic::{self, AssertUnwindSafe},
    process::{Command, ExitStatus, Output, Stdio},
    thread,
};
#[cfg(unix)]
use std::{net::TcpStream, process::Child};

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
}

pub(crate) const MANIFEST_FILE_NAME: &str = "install-manifest.txt";
const SHORT_LINE_SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
// TODO-RAINCLAUDE: the leading dash catches a command that is given the
// path without `--` before it. For example, `mkdir -p "$install_dir"` would
// run as `mkdir -p "-relative dir"`, and mkdir would read that as options.
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
    // TODO-RAINCLAUDE: Unix-only because the one test using it sends a
    // signal. StallServer says what stalling means.
    #[cfg(unix)]
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
    // TODO-RAINCLAUDE: both scripts must compare checksums ignoring case.
    UppercaseChecksum,
    PlainHttpArchiveUrl,
    // TODO-RAINCLAUDE: an archive line cut off before its URL.
    ShortArchiveLine,
    MalformedVersion,
    UnknownStatus,
    UnknownKey,
    EmptyBody,
}

/// The ways in which scripts can be started.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode {
    Args,
    ArgsWithEquals,
    /// Simulate `curl | sh`, with options specified as environment variables.
    Piped,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PowerShellMode {
    // TODO-RAINCLAUDE: run the script file with options, through the call
    // operator, as someone who downloaded it would from a prompt. For
    // example:
    //
    //     powershell -Command "<trust setup>; & 'C:\...\install.ps1' -Version '0.9'"
    Args,
    // TODO-RAINCLAUDE: run the script file with `powershell -File`. For
    // example:
    //
    //     powershell -File C:\...\install.ps1 -Version 0.9
    //
    // Nothing can run before the script in this launch, so there is no way
    // to make PowerShell accept the test certificate: only for cases that
    // fail before any download.
    File,
    /// Simulate `irm | iex`, with options specified as environment variables.
    Piped,
    /// Fetch the script and invoke it as a script block with parameters.
    ScriptBlock,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Launch {
    Sh(Mode),
    PowerShell(PowerShellMode),
}

impl Launch {
    fn kind(self) -> ShellKind {
        match self {
            Self::Sh(_) => ShellKind::Sh,
            Self::PowerShell(_) => ShellKind::PowerShell,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum InstallDirSource {
    Explicit,
    /// Pass an explicit install directory with square brackets in its name.
    ExplicitWithBrackets,
    ExplicitRelative,
    CargoHome,
    Home,
    #[cfg(not(windows))]
    Unset,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DefaultBaseUrl {
    Server,
    // TODO-RAINCLAUDE: an address nothing answers at, for a test that passes
    // a base URL. For example, a script that ignored `--base-url` would ask
    // get.widget.example for the manifest and fail. If the default were the
    // server too, that script would still pass.
    Elsewhere,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum VersionChoice {
    Default,
    Explicit(&'static str),
}

impl VersionChoice {
    // TODO-RAINCLAUDE: the directory on the server whose manifest the script
    // should request. For example, for `0.9` the script requests
    // <base URL>/0.9/install-manifest.txt.
    pub(crate) fn manifest_spec(self) -> &'static str {
        match self {
            Self::Default => "latest",
            Self::Explicit(version) => version,
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

#[derive(Clone, Debug)]
pub(crate) struct Invocation<'a> {
    pub(crate) version: VersionChoice,
    pub(crate) base_url: Option<String>,
    pub(crate) target: Option<Target>,
    pub(crate) launch: Launch,
    pub(crate) install_dir: InstallDirSource,
    /// Environment variables to set (Some) or remove (None) for the script.
    ///
    /// These are applied last.
    pub(crate) env: Vec<(&'a str, Option<String>)>,
    /// Replace `PATH` when set.
    pub(crate) path: Option<Utf8PathBuf>,
    pub(crate) helper_exit: Option<u8>,
    /// For piped `sh` only -- the bytes to feed to the shell instead of the
    /// whole script, to simulate a truncated download.
    pub(crate) stdin: Option<Vec<u8>>,
    // TODO-RAINCLAUDE: for launches that run the script file with arguments
    // only: extra arguments passed after the options, as is.
    pub(crate) extra_args: &'a [&'a str],
}

impl Invocation<'_> {
    // TODO-RAINCLAUDE: the latest version into the explicit install directory,
    // with nothing else set; callers override fields with struct update
    // syntax.
    pub(crate) fn latest(base_url: Option<String>, launch: Launch) -> Self {
        Self {
            version: VersionChoice::Default,
            base_url,
            target: None,
            launch,
            install_dir: InstallDirSource::Explicit,
            env: Vec::new(),
            path: None,
            helper_exit: None,
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

// TODO-RAINCLAUDE: the names of the entries in the directory, sorted; empty
// if the directory does not exist. Any other failure to read it is an error,
// so that it cannot pass for "nothing was left behind".
pub(crate) fn dir_entry_names(dir: &Utf8Path) -> Result<Vec<String>> {
    let entries = match dir.read_dir_utf8() {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).wrap_err_with(|| format!("failed to read {dir}")),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.wrap_err_with(|| format!("failed to read {dir}"))?;
        names.push(entry.file_name().to_owned());
    }
    names.sort();
    Ok(names)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    Server,
    ServerRedirect,
    // TODO-RAINCLAUDE: a bare TCP server with a TLS front of its own, since
    // archive URLs must be https.
    HangUp {
        server: HangUpServer,
        front: TlsFront,
    },
    #[cfg(unix)]
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
            #[cfg(unix)]
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
        let mut flags: Vec<&str> = post_install.script_flags();
        let server_url = fixture.base_url();
        match default_base_url {
            DefaultBaseUrl::Elsewhere => {}
            DefaultBaseUrl::Server => {
                let position = flags
                    .iter()
                    .position(|flag| *flag == BASE_URL.sh_flag)
                    .ok_or_else(|| eyre!("the script flags do not set a base URL"))?;
                flags[position + 1] = &server_url;
            }
        }
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
            DefaultBaseUrl::Elsewhere,
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
            #[cfg(unix)]
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
            ManifestTweak::UppercaseChecksum => manifest
                .lines()
                .map(
                    |line| match line.split(' ').collect::<Vec<_>>().as_slice() {
                        ["archive", target, format, sha256, url] => {
                            format!(
                                "archive {target} {format} {} {url}\n",
                                sha256.to_ascii_uppercase()
                            )
                        }
                        _ => format!("{line}\n"),
                    },
                )
                .collect(),
            ManifestTweak::PlainHttpArchiveUrl => {
                set_field("archive", 4, "http://archives.widget.example/widget")
            }
            ManifestTweak::ShortArchiveLine => {
                format!("{manifest}archive made-up-target tar.gz {SHORT_LINE_SHA256}\n")
            }
            ManifestTweak::MalformedVersion => set_field("version", 1, "0.9"),
            ManifestTweak::UnknownStatus => set_field("status", 1, "retired"),
            // TODO-RAINCLAUDE: a key added by a later mukti, which older
            // scripts must skip. It goes directly after the header, so that a
            // script that stopped reading at an unknown key would miss
            // everything.
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
            // TODO-RAINCLAUDE: the test waits for this request itself.
            #[cfg(unix)]
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

    // TODO-RAINCLAUDE: returns the base URL under which the redirect is
    // served.
    pub(crate) fn expect_manifest_redirect_to_http(&self) -> String {
        let path = format!("/latest/{MANIFEST_FILE_NAME}");
        self.expect_get(
            format!("/to-http{path}"),
            1,
            status_code(302).insert_header(
                "Location",
                format!("{}{path}", self.server.plain_http_url()),
            ),
        );
        format!("{}/to-http", self.base_url())
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

    // TODO-RAINCLAUDE: call with the result of a test's body once the script
    // has run. Checks the HTTP requests whether or not the body succeeded (the
    // server would otherwise panic on drop, replacing the body's own error),
    // and adds what it finds to the body's problems or error.
    pub(crate) fn verify_requests_after<T>(
        &mut self,
        result: Result<(Vec<String>, T)>,
    ) -> Result<(Vec<String>, T)> {
        let mut request_problems = Vec::new();
        self.verify_requests(&mut request_problems);
        match result {
            Ok((mut problems, value)) => {
                problems.extend(request_problems);
                Ok((problems, value))
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

    pub(crate) fn take_hang_up_count(&self) -> Option<usize> {
        match &self.archive_origin {
            ArchiveOrigin::Server | ArchiveOrigin::ServerRedirect => None,
            ArchiveOrigin::HangUp { server, .. } => Some(server.take_hang_up_count()),
            #[cfg(unix)]
            ArchiveOrigin::Stall { .. } => None,
        }
    }

    #[cfg(unix)]
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

    pub(crate) fn stray_work_dir_entries(&self, source: InstallDirSource) -> Result<Vec<String>> {
        let work_dir = self.work_dir();
        let install_dir = self.install_dir(source);
        let mut names = dir_entry_names(&work_dir)?;
        names.retain(|name| work_dir.join(name) != install_dir);
        Ok(names)
    }

    // TODO-RAINCLAUDE: install.ps1 reads PowerShell's $HOME, which is fixed
    // when PowerShell starts and can only be redirected through the
    // environment. Off Windows, pwsh takes it from HOME. On Windows, with HOME
    // and USERPROFILE both set as here, Windows PowerShell 5.1 and PowerShell 7
    // both report this directory (checked by hand).
    //
    // USERPROFILE is left unset off Windows, so that a script reading it there
    // finds nothing.
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

    pub(crate) fn install_dir(&self, source: InstallDirSource) -> Utf8PathBuf {
        match source {
            InstallDirSource::Explicit => self.temp.path().join("install dir"),
            InstallDirSource::ExplicitWithBrackets => self.temp.path().join("in[stall]"),
            InstallDirSource::ExplicitRelative => self.work_dir().join(RELATIVE_INSTALL_DIR),
            InstallDirSource::CargoHome => self.cargo_home().join("bin"),
            InstallDirSource::Home => self.home().join(".cargo").join("bin"),
            #[cfg(not(windows))]
            InstallDirSource::Unset => self.temp.path().join("install dir"),
        }
    }

    /// Returns the paths that must not exist after a run.
    ///
    /// This includes:
    ///
    /// * If the source doesn't specify them, the default location.
    /// * The real paths on the machine (we override environment variables to ensure this).
    pub(crate) fn forbidden_paths(&self, source: InstallDirSource) -> Vec<Utf8PathBuf> {
        let cargo_home = self.cargo_home();
        let home_cargo = self.home().join(".cargo");
        let mut paths = match source {
            InstallDirSource::Explicit
            | InstallDirSource::ExplicitWithBrackets
            | InstallDirSource::ExplicitRelative => {
                vec![cargo_home, home_cargo]
            }
            InstallDirSource::CargoHome => vec![home_cargo],
            InstallDirSource::Home => vec![cargo_home],
            #[cfg(not(windows))]
            InstallDirSource::Unset => vec![cargo_home, home_cargo],
        };
        paths.extend(real_default_binaries());
        paths
    }

    pub(crate) fn run(
        &self,
        shell: &ShellSpec,
        shell_path: &Utf8Path,
        invocation: &Invocation<'_>,
    ) -> Result<Outcome> {
        let (command, stdin) = self.command(shell, shell_path, invocation)?;
        run_to_completion(command, stdin, shell_path)
    }

    #[cfg(unix)]
    pub(crate) fn spawn(
        &self,
        shell: &ShellSpec,
        shell_path: &Utf8Path,
        invocation: &Invocation<'_>,
    ) -> Result<Child> {
        let (mut command, stdin) = self.command(shell, shell_path, invocation)?;
        if stdin.is_some() {
            bail!("spawn does not support feeding the script on stdin");
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .wrap_err_with(|| format!("failed to run {shell_path}"))
    }

    fn command(
        &self,
        shell: &ShellSpec,
        shell_path: &Utf8Path,
        invocation: &Invocation<'_>,
    ) -> Result<(Command, Option<Vec<u8>>)> {
        // Both PowerShell and Git Bash on Windows accept forward slashes.
        let install_dir = self
            .install_dir(invocation.install_dir)
            .as_str()
            .replace('\\', "/");

        let mut options = Vec::new();
        if let Some(base_url) = &invocation.base_url {
            options.push((BASE_URL, base_url.clone()));
        }
        match invocation.version {
            VersionChoice::Default => {}
            VersionChoice::Explicit(version) => options.push((VERSION, version.to_owned())),
        }
        match invocation.install_dir {
            InstallDirSource::Explicit | InstallDirSource::ExplicitWithBrackets => {
                options.push((INSTALL_DIR, install_dir));
            }
            InstallDirSource::ExplicitRelative => {
                options.push((INSTALL_DIR, RELATIVE_INSTALL_DIR.to_owned()));
            }
            InstallDirSource::CargoHome | InstallDirSource::Home => {}
            #[cfg(not(windows))]
            InstallDirSource::Unset => {}
        }
        if let Some(target) = invocation.target {
            options.push((TARGET, target.as_str().to_owned()));
        }

        let mut command = Command::new(shell_path);
        // TODO-RAINCLAUDE: scripts run in the work dir, which relative install
        // directories resolve against and which must be left clean.
        //
        // PowerShell keeps its own location, separate from the process's
        // working directory. For example, in a PowerShell started in
        // C:\Users\me, after `cd D:\work`, `-InstallDir bin` must mean
        // D:\work\bin, but a .NET call such as
        // [IO.Directory]::CreateDirectory('bin') would create
        // C:\Users\me\bin. So that a test can tell the two apart, the piped
        // and script block launches start in the fixture root and then set
        // their location to the work dir.
        let work_dir = self.work_dir();
        command.current_dir(match invocation.launch {
            Launch::Sh(Mode::Args | Mode::ArgsWithEquals | Mode::Piped)
            | Launch::PowerShell(PowerShellMode::Args | PowerShellMode::File) => work_dir.clone(),
            Launch::PowerShell(PowerShellMode::Piped | PowerShellMode::ScriptBlock) => {
                self.temp.path().to_owned()
            }
        });
        command.args(shell.args);
        let script = self.script(invocation.launch.kind());
        let script_url = format!("{}/install.ps1", self.base_url());
        let trust = match invocation.launch {
            Launch::Sh(Mode::Args | Mode::ArgsWithEquals | Mode::Piped)
            | Launch::PowerShell(PowerShellMode::File) => String::new(),
            Launch::PowerShell(
                PowerShellMode::Args | PowerShellMode::Piped | PowerShellMode::ScriptBlock,
            ) => self.server.powershell_trust(shell.name)?,
        };
        let powershell = ["-NoProfile", "-ExecutionPolicy", "Bypass"];
        let mut stdin = None;
        let mut option_env = Vec::new();
        let (takes_extra_args, takes_stdin) = match invocation.launch {
            Launch::Sh(Mode::Args | Mode::ArgsWithEquals)
            | Launch::PowerShell(PowerShellMode::Args | PowerShellMode::File) => (true, false),
            Launch::Sh(Mode::Piped) => (false, true),
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
        match invocation.launch {
            Launch::Sh(Mode::Args) => {
                command.arg(script.as_str().replace('\\', "/"));
                for (option, value) in &options {
                    command.args([option.sh_flag, value]);
                }
                command.args(invocation.extra_args);
            }
            Launch::Sh(Mode::ArgsWithEquals) => {
                command.arg(script.as_str().replace('\\', "/"));
                for (option, value) in &options {
                    command.arg(format!("{}={value}", option.sh_flag));
                }
                command.args(invocation.extra_args);
            }
            Launch::Sh(Mode::Piped) => {
                stdin = Some(match &invocation.stdin {
                    Some(bytes) => bytes.clone(),
                    None => {
                        fs::read(&script).wrap_err_with(|| format!("failed to read {script}"))?
                    }
                });
                option_env = options;
            }
            Launch::PowerShell(PowerShellMode::Args) => {
                let parameters: Vec<_> = options
                    .iter()
                    .map(|(option, value)| format!("{} '{value}'", option.powershell_parameter))
                    .collect();
                let invoke = format!(
                    "{trust}& '{script}' {} {}",
                    parameters.join(" "),
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
                // TODO-RAINCLAUDE: the check for leftover functions looks in
                // the Function: drive. Asking Get-Command for a name that
                // does not exist makes Windows PowerShell search every
                // installed module for it, which took over a minute on the
                // GitHub runners.
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
                option_env = options;
            }
            Launch::PowerShell(PowerShellMode::ScriptBlock) => {
                let parameters: Vec<_> = options
                    .iter()
                    .map(|(option, value)| format!("{} '{value}'", option.powershell_parameter))
                    .collect();
                let invoke = format!(
                    "{trust}Set-Location -LiteralPath '{work_dir}'; \
                     & ([scriptblock]::Create((irm '{script_url}'))) {}",
                    parameters.join(" ")
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
        for (option, value) in option_env {
            command.env(format!("{ENV_PREFIX}_{}", option.env_suffix), value);
        }
        match invocation.install_dir {
            InstallDirSource::Explicit
            | InstallDirSource::ExplicitWithBrackets
            | InstallDirSource::ExplicitRelative
            | InstallDirSource::CargoHome => {
                command.env("CARGO_HOME", self.cargo_home());
                self.set_home(&mut command);
            }
            InstallDirSource::Home => self.set_home(&mut command),
            #[cfg(not(windows))]
            InstallDirSource::Unset => {}
        }
        command.env(HELPER_LOG_VAR, self.helper_log());
        if let Some(code) = invocation.helper_exit {
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
