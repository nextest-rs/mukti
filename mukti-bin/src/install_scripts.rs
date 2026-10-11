// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    atomic::write_atomic,
    errors::{InvalidNumber, InvalidNumberKind, InvalidScriptValue},
    manifests::{MANIFEST_FILE_NAME, MANIFEST_FORMAT_VERSION, MANIFEST_HEADER},
};
use camino::Utf8Path;
use color_eyre::eyre::{bail, Result};
use semver::Version;
use std::{fmt, str::FromStr};

const INSTALL_SH_TEMPLATE: &str = include_str!("templates/install.sh");
const INSTALL_PS1_TEMPLATE: &str = include_str!("templates/install.ps1");

// Note that the values below are embedded in shell and PowerShell source.

// Only accept ASCII letters, digits, `.`, `_`, and `-`, starting with a letter
// or digit.
fn is_script_name(input: &str) -> bool {
    input.len() <= 64
        && input.starts_with(|c: char| c.is_ascii_alphanumeric())
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectName(String);

impl FromStr for ProjectName {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if !is_script_name(input) {
            return Err(InvalidScriptValue::new(
                "project name",
                input,
                "1 to 64 ASCII letters, digits, `.`, `_`, or `-`, starting with a letter or digit",
            ));
        }
        Ok(Self(input.to_owned()))
    }
}

/// The name of the binary inside each archive, without `.exe`.
#[derive(Clone, Debug)]
pub(crate) struct BinName(String);

impl FromStr for BinName {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let has_exe_suffix = input.to_ascii_lowercase().ends_with(".exe");
        if !is_script_name(input) || has_exe_suffix {
            return Err(InvalidScriptValue::new(
                "binary name",
                input,
                "1 to 64 ASCII letters, digits, `.`, `_`, or `-`, starting with a letter or digit, and without a `.exe` suffix, which the scripts add on Windows",
            ));
        }
        Ok(Self(input.to_owned()))
    }
}

/// The prefix for the scripts' environment variables.
///
/// This becomes part of a variable name, so it must be an identifier.
#[derive(Clone, Debug)]
pub(crate) struct EnvPrefix(String);

impl FromStr for EnvPrefix {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let valid = input.len() <= 64
            && input.starts_with(|c: char| c.is_ascii_uppercase())
            && !input.ends_with('_')
            && input
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !valid {
            return Err(InvalidScriptValue::new(
                "environment variable prefix",
                input,
                "1 to 64 ASCII uppercase letters, digits, or `_`, starting with a letter and not ending with `_`",
            ));
        }
        Ok(Self(input.to_owned()))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HttpsUrl(String);

impl HttpsUrl {
    fn without_trailing_slashes(&self) -> &str {
        self.0.trim_end_matches('/')
    }
}

impl FromStr for HttpsUrl {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let valid = input.len() <= 256
            && input.strip_prefix("https://").is_some_and(|rest| {
                rest.starts_with(|c: char| c.is_ascii_alphanumeric())
                    && rest.chars().all(|c| {
                        c.is_ascii_alphanumeric()
                            || matches!(c, '.' | '-' | '_' | '~' | '/' | ':' | '#')
                    })
            });
        if !valid {
            return Err(InvalidScriptValue::new(
                "URL",
                input,
                "an https:// URL of at most 256 characters, using only ASCII letters, digits, and `.-_~/:#`",
            ));
        }
        Ok(Self(input.to_owned()))
    }
}

/// The URL manifests are served from. The scripts append a path to it, so it
/// cannot carry a fragment.
#[derive(Clone, Debug)]
pub(crate) struct BaseUrl(HttpsUrl);

impl FromStr for BaseUrl {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || {
            InvalidScriptValue::new(
                "base URL",
                input,
                "an https:// URL of at most 256 characters, using only ASCII letters, digits, and `.-_~/:`",
            )
        };
        if input.contains('#') {
            return Err(invalid());
        }
        input.parse().map(Self).map_err(|_| invalid())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GlibcVersion {
    major: u32,
    minor: u32,
}

impl FromStr for GlibcVersion {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid =
            || InvalidScriptValue::new("glibc version", input, "MAJOR.MINOR, for example 2.27");
        let (major, minor) = input.split_once('.').ok_or_else(invalid)?;
        Ok(Self {
            major: parse_number(major).map_err(|error| invalid().with_cause(error))?,
            minor: parse_number(minor).map_err(|error| invalid().with_cause(error))?,
        })
    }
}

/// A version with no prerelease or build metadata.
#[derive(Clone, Debug)]
pub(crate) struct StableVersion(Version);

impl FromStr for StableVersion {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.parse::<Version>() {
            Ok(version) if version.pre.is_empty() && version.build.is_empty() => Ok(Self(version)),
            Ok(_) | Err(_) => Err(InvalidScriptValue::new(
                "version",
                input,
                "MAJOR.MINOR.PATCH with no prerelease or build metadata, for example 0.9.62",
            )),
        }
    }
}

/// Similar to `u32::from_str`, except it only accepts numbers in canonical
/// form.
fn parse_number(input: &str) -> Result<u32, InvalidNumber> {
    let value: u32 = input
        .parse()
        .map_err(|error| InvalidNumber::new(input, InvalidNumberKind::ParseInt(error)))?;
    if value.to_string() != input {
        return Err(InvalidNumber::new(
            input,
            InvalidNumberKind::NotCanonical { canonical: value },
        ));
    }
    Ok(value)
}

/// The arguments for the command run after installing.
///
/// This is a non-empty list of arguments.
#[derive(Clone, Debug)]
pub(crate) struct PostInstallArgs(Vec<String>);

impl PostInstallArgs {
    // TODO-RAINCLAUDE: for example, "self setup" becomes `'self' 'setup'` in
    // install.sh and `'self', 'setup'` in install.ps1. Single quotes are safe
    // in both only because the allowlist in from_str excludes `'`: an
    // argument such as `it's` would end the quoted string early. Keep the two
    // together.
    fn quoted_list(&self, kind: ScriptKind) -> String {
        let separator = match kind {
            ScriptKind::Sh => " ",
            ScriptKind::PowerShell => ", ",
        };
        let quoted: Vec<_> = self.0.iter().map(|arg| format!("'{arg}'")).collect();
        quoted.join(separator)
    }
}

impl FromStr for PostInstallArgs {
    type Err = InvalidScriptValue;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let args: Vec<_> = input.split_ascii_whitespace().map(str::to_owned).collect();
        let valid = !args.is_empty()
            && args.len() <= 16
            && args.iter().all(|arg| {
                arg.len() <= 64
                    && arg.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '=' | ':' | '/')
                    })
            });
        if !valid {
            return Err(InvalidScriptValue::new(
                "post-install arguments",
                input,
                "1 to 16 space-separated arguments, each of at most 64 characters, using only ASCII letters, digits, and `._-=:/`",
            ));
        }
        Ok(Self(args))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PostInstall {
    pub(crate) args: PostInstallArgs,
    /// The post-install step is only run for releases at or above this version.
    pub(crate) min_version: StableVersion,
}

#[derive(Clone, Debug)]
pub(crate) struct InstallScriptConfig {
    pub(crate) project: ProjectName,
    pub(crate) bin_name: BinName,
    pub(crate) base_url: BaseUrl,
    pub(crate) docs_url: HttpsUrl,
    pub(crate) env_prefix: EnvPrefix,
    pub(crate) min_glibc: GlibcVersion,
    pub(crate) post_install: Option<PostInstall>,
}

#[derive(Clone, Copy, Debug)]
enum ScriptKind {
    Sh,
    PowerShell,
}

impl ScriptKind {
    const ALL: [Self; 2] = [Self::Sh, Self::PowerShell];

    fn file_name(self) -> &'static str {
        match self {
            Self::Sh => "install.sh",
            Self::PowerShell => "install.ps1",
        }
    }

    fn template(self) -> &'static str {
        match self {
            Self::Sh => INSTALL_SH_TEMPLATE,
            Self::PowerShell => INSTALL_PS1_TEMPLATE,
        }
    }
}

impl fmt::Display for ScriptKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.file_name())
    }
}

pub(crate) fn generate_install_scripts(
    config: &InstallScriptConfig,
    out_dir: &Utf8Path,
) -> Result<()> {
    let scripts = ScriptKind::ALL
        .into_iter()
        .map(|kind| Ok((kind, render(kind, config)?)))
        .collect::<Result<Vec<_>>>()?;

    for (kind, contents) in scripts {
        let path = out_dir.join(kind.file_name());
        write_atomic(&path, contents.as_bytes())?;
        eprintln!("wrote {path}");
    }

    Ok(())
}

fn render(kind: ScriptKind, config: &InstallScriptConfig) -> Result<String> {
    let no_post_install = Version::new(0, 0, 0);
    let (post_install_args, min_version) = match &config.post_install {
        Some(post_install) => (
            post_install.args.quoted_list(kind),
            &post_install.min_version.0,
        ),
        None => (String::new(), &no_post_install),
    };

    let mut substitutions = vec![
        ("PROJECT", config.project.0.clone()),
        ("BIN_NAME", config.bin_name.0.clone()),
        (
            "BASE_URL",
            config.base_url.0.without_trailing_slashes().to_owned(),
        ),
        ("DOCS_URL", config.docs_url.0.clone()),
        ("ENV_PREFIX", config.env_prefix.0.clone()),
        ("POST_INSTALL_ARGS", post_install_args),
        ("POST_INSTALL_MIN_MAJOR", min_version.major.to_string()),
        ("POST_INSTALL_MIN_MINOR", min_version.minor.to_string()),
        ("POST_INSTALL_MIN_PATCH", min_version.patch.to_string()),
        ("MANIFEST_HEADER", MANIFEST_HEADER.to_owned()),
        ("MANIFEST_FORMAT", MANIFEST_FORMAT_VERSION.to_string()),
        ("MANIFEST_FILE_NAME", MANIFEST_FILE_NAME.to_owned()),
    ];
    match kind {
        ScriptKind::Sh => {
            substitutions.push(("GLIBC_MIN_MAJOR", config.min_glibc.major.to_string()));
            substitutions.push(("GLIBC_MIN_MINOR", config.min_glibc.minor.to_string()));
        }
        // glibc is not relevant on Windows.
        ScriptKind::PowerShell => {}
    }

    substitute(kind, kind.template(), &substitutions)
}

/// Substitutes placeholders in the install templates with the real values.
///
/// Values cannot contain `@`, so one substitution can never create a
/// placeholder for a later one.
fn substitute(
    kind: ScriptKind,
    template: &str,
    substitutions: &[(&str, String)],
) -> Result<String> {
    let mut out = template.to_owned();
    for (name, value) in substitutions {
        let placeholder = format!("@{name}@");
        if !out.contains(&placeholder) {
            bail!("the {kind} template has no {placeholder} placeholder (this is a bug in mukti)");
        }
        out = out.replace(&placeholder, value);
    }
    if let Some(leftover) = find_placeholder(&out) {
        bail!("the {kind} template has an unknown placeholder {leftover} (this is a bug in mukti)");
    }
    Ok(out)
}

/// Finds the first `@NAME@`, where `NAME` is uppercase letters, digits, and
/// underscores.
fn find_placeholder(text: &str) -> Option<&str> {
    let mut rest = text;
    while let Some(start) = rest.find('@') {
        let after = &rest[start + 1..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(after.len());
        if name_len > 0 && after[name_len..].starts_with('@') {
            return Some(&rest[start..start + name_len + 2]);
        }
        rest = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(post_install: Option<PostInstall>) -> InstallScriptConfig {
        InstallScriptConfig {
            project: "my-project".parse().expect("valid name"),
            bin_name: "my-bin".parse().expect("valid name"),
            base_url: "https://get.example.com/".parse().expect("valid URL"),
            docs_url: "https://example.com/docs/install/#binaries"
                .parse()
                .expect("valid URL"),
            env_prefix: "MY_INSTALL".parse().expect("valid prefix"),
            min_glibc: "2.17".parse().expect("valid glibc version"),
            post_install,
        }
    }

    fn post_install() -> PostInstall {
        PostInstall {
            args: "self setup --source=install-script"
                .parse()
                .expect("valid arguments"),
            min_version: "1.2.3".parse().expect("valid version"),
        }
    }

    #[test]
    fn templates_have_unix_line_endings() {
        for kind in ScriptKind::ALL {
            assert!(
                !kind.template().contains('\r'),
                "the {kind} template has LF line endings",
            );
        }
    }

    #[test]
    fn render_substitutes_every_placeholder() {
        let config = config(Some(post_install()));

        let sh = render(ScriptKind::Sh, &config).expect("install.sh rendered");
        assert_eq!(find_placeholder(&sh), None);
        assert!(sh.contains("PROJECT='my-project'\n"), "project is set");
        assert!(sh.contains("BIN_NAME='my-bin'\n"), "binary name is set");
        assert!(
            sh.contains("DEFAULT_BASE_URL='https://get.example.com'\n"),
            "base URL is set, without its trailing slash",
        );
        assert!(
            sh.contains("${MY_INSTALL_VERSION:-latest}"),
            "prefix is used"
        );
        assert!(sh.contains("GLIBC_MIN_MAJOR=2\nGLIBC_MIN_MINOR=17\n"));
        assert!(sh.contains("set -- 'self' 'setup' '--source=install-script'\n"));
        assert!(sh.contains(
            "POST_INSTALL_MIN_MAJOR=1\nPOST_INSTALL_MIN_MINOR=2\nPOST_INSTALL_MIN_PATCH=3\n"
        ));

        let ps1 = render(ScriptKind::PowerShell, &config).expect("install.ps1 rendered");
        assert_eq!(find_placeholder(&ps1), None);
        assert!(ps1.contains("$project = 'my-project'\n"));
        assert!(ps1.contains("$env:MY_INSTALL_VERSION"));
        assert!(ps1.contains("$postInstallArgs = @('self', 'setup', '--source=install-script')\n"));
        assert!(ps1.contains("$postInstallMinVersion = @(1, 2, 3)\n"));
    }

    #[test]
    fn render_without_post_install() {
        let config = config(None);
        let sh = render(ScriptKind::Sh, &config).expect("install.sh rendered");
        assert!(sh.contains("    set -- \n"), "no post-install arguments");
        let ps1 = render(ScriptKind::PowerShell, &config).expect("install.ps1 rendered");
        assert!(ps1.contains("$postInstallArgs = @()\n"));
    }

    #[test]
    fn substitute_rejects_template_drift() {
        let missing = substitute(ScriptKind::Sh, "no placeholders", &[("A", "x".to_owned())])
            .expect_err("missing placeholder rejected");
        assert!(missing.to_string().contains("has no @A@ placeholder"));

        let leftover = substitute(ScriptKind::Sh, "@A@ and @B_2@", &[("A", "x".to_owned())])
            .expect_err("leftover placeholder rejected");
        assert!(leftover.to_string().contains("unknown placeholder @B_2@"));

        // Have to be a bit careful, because sh's "$@" and PowerShell's @(...)
        // and @{...} are not placeholders.
        assert_eq!(
            find_placeholder("\"$@\" @(1, 2) @{} a@b.c @lower@ @@"),
            None
        );
    }

    #[test]
    fn glibc_version_errors_say_what_is_wrong() {
        let prefix = |input: &str| {
            format!(
                "invalid glibc version {input:?} for a generated install script \
                 (expected MAJOR.MINOR, for example 2.27)"
            )
        };
        let cases = [
            ("2", String::new()),
            (
                "02.27",
                ": \"02\" is not in canonical form (expected 2)".to_owned(),
            ),
            (
                "2.+27",
                ": \"+27\" is not in canonical form (expected 27)".to_owned(),
            ),
            (
                "2.x",
                ": \"x\" is not a number: invalid digit found in string".to_owned(),
            ),
            (
                "2.",
                ": \"\" is not a number: cannot parse integer from empty string".to_owned(),
            ),
            (
                "99999999999.1",
                ": \"99999999999\" is not a number: number too large to fit in target type"
                    .to_owned(),
            ),
        ];
        for (input, cause) in cases {
            let error = input
                .parse::<GlibcVersion>()
                .expect_err("invalid glibc version rejected");
            assert_eq!(error.to_string(), format!("{}{cause}", prefix(input)));
        }
    }

    #[test]
    fn script_values_accept_the_longest_allowed_input() {
        let name = "a".repeat(64);
        name.parse::<ProjectName>()
            .expect("64-character project name accepted");
        name.parse::<BinName>()
            .expect("64-character binary name accepted");
        "A".repeat(64)
            .parse::<EnvPrefix>()
            .expect("64-character prefix accepted");

        let url = format!("https://{}", "a".repeat(256 - "https://".len()));
        url.parse::<HttpsUrl>().expect("256-character URL accepted");
        url.parse::<BaseUrl>()
            .expect("256-character base URL accepted");
        format!("{url}a")
            .parse::<HttpsUrl>()
            .expect_err("257-character URL rejected");

        let args = vec![name.as_str(); 16].join(" ");
        args.parse::<PostInstallArgs>()
            .expect("16 arguments of 64 characters accepted");
    }

    #[test]
    fn script_values_reject_unsafe_input() {
        // Anything that could end a quoted string or start an expansion, in sh
        // or PowerShell.
        let injections = [
            "", "a b", "a'b", "a\"b", "a$b", "a`b", "a\\b", "a;b", "a|b", "a&b", "a(b", "a)b",
            "a<b", "a>b", "a\nb", "a\rb", "a\tb", "a@b", "a{b", "a}b", "a*b", "a?b", "a[b", "a!b",
            "a%b", "a,b", "\u{e9}",
        ];
        for input in injections {
            assert!(input.parse::<ProjectName>().is_err(), "project {input:?}");
            assert!(input.parse::<BinName>().is_err(), "binary {input:?}");
            assert!(input.parse::<EnvPrefix>().is_err(), "prefix {input:?}");
            assert!(input.parse::<GlibcVersion>().is_err(), "glibc {input:?}");
            assert!(input.parse::<StableVersion>().is_err(), "version {input:?}");
            let url = format!("https://example.com/{input}");
            if !input.is_empty() {
                assert!(url.parse::<HttpsUrl>().is_err(), "URL {url:?}");
            }
            // ASCII whitespace only separates arguments, so it is not an error
            // here.
            if !input.contains([' ', '\n', '\r', '\t']) {
                assert!(input.parse::<PostInstallArgs>().is_err(), "args {input:?}");
            }
        }

        for input in ["-leading-dash", ".hidden", "_x", &"a".repeat(65)] {
            assert!(input.parse::<ProjectName>().is_err(), "project {input:?}");
            assert!(input.parse::<BinName>().is_err(), "binary {input:?}");
        }
        // The scripts add `.exe` themselves, so a binary name must not have it.
        // (The project name is allowed to.)
        for input in ["widget.exe", "widget.EXE", "widget.Exe"] {
            assert!(input.parse::<BinName>().is_err(), "binary {input:?}");
            assert!(input.parse::<ProjectName>().is_ok(), "project {input:?}");
        }
        for input in ["widget", "cargo-nextest", "a.b_c-1", "exe", "widget.exec"] {
            assert!(input.parse::<BinName>().is_ok(), "binary {input:?}");
        }
        for input in ["lower", "1ABC", "_ABC", "ABC_", "A-B", &"A".repeat(65)] {
            assert!(input.parse::<EnvPrefix>().is_err(), "prefix {input:?}");
        }
        for input in [
            "http://example.com",
            "https://",
            "https:///x",
            "ftp://example.com",
            "example.com",
        ] {
            assert!(input.parse::<HttpsUrl>().is_err(), "URL {input:?}");
            assert!(input.parse::<BaseUrl>().is_err(), "base URL {input:?}");
        }
        // A fragment is fine in a documentation link, but not in the URL that
        // paths are appended to.
        "https://example.com/docs#install"
            .parse::<HttpsUrl>()
            .expect("valid documentation URL");
        for input in ["https://example.com/#x", "https://example.com/?a"] {
            assert!(input.parse::<BaseUrl>().is_err(), "base URL {input:?}");
        }
        for input in [
            "2",
            "2.",
            ".27",
            "2.27.1",
            "+2.27",
            "02.27",
            "2.027",
            "2.-1",
            "2.x",
            "99999999999.1",
        ] {
            assert!(input.parse::<GlibcVersion>().is_err(), "glibc {input:?}");
        }
        for input in [
            "0.9",
            "0.9.62.1",
            "0.9.62-rc.1",
            "0.9.62+build",
            "v0.9.62",
            "0.9.+62",
            "0.9.062",
        ] {
            assert!(input.parse::<StableVersion>().is_err(), "version {input:?}");
        }
        for input in ["   ", &"a ".repeat(17), &"a".repeat(65)] {
            assert!(input.parse::<PostInstallArgs>().is_err(), "args {input:?}");
        }
    }
}
