// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    shells::PowerShellName,
    tls::{TlsFront, TlsIdentity},
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{Context, Result};
use httptest::{Expectation, Server};
use std::{fs, net::SocketAddr, process::Command};

// TODO-RAINCLAUDE: the server a fixture's scripts fetch from: an http server
// that the test puts expectations on, reached through a TLS front. Scripts
// only ever see the https address.
pub(crate) struct TestServer {
    http: Server,
    identity: TlsIdentity,
    front: TlsFront,
    // TODO-RAINCLAUDE: holds a curlrc that names the test CA. An environment
    // variable would be simpler, but curl built for Schannel (Git for Windows
    // on ARM64) ignores CURL_CA_BUNDLE and SSL_CERT_FILE; it does read a
    // curlrc.
    curl_home: Utf8PathBuf,
}

impl TestServer {
    // TODO-RAINCLAUDE: writes the files that tell curl about the test CA into
    // `dir`.
    pub(crate) fn start(dir: &Utf8Path) -> Result<Self> {
        let http = Server::run();
        let identity = TlsIdentity::generate()?;
        let front = TlsFront::start(&identity, http.addr())?;

        let ca_file = dir.join("test-ca.pem");
        fs::write(&ca_file, identity.ca_pem())
            .wrap_err_with(|| format!("failed to write {ca_file}"))?;
        let curl_home = dir.join("curl home");
        fs::create_dir_all(&curl_home).wrap_err_with(|| format!("failed to create {curl_home}"))?;
        let curlrc = curl_home.join(".curlrc");
        // TODO-RAINCLAUDE: ssl-no-revoke is for curl built for Schannel, which
        // otherwise refuses the server's certificate because it names no
        // place to check for revocation ("the revocation status is unknown").
        // Other builds accept the option and ignore it.
        fs::write(
            &curlrc,
            format!(
                "cacert = \"{}\"\nssl-no-revoke\n",
                ca_file.as_str().replace('\\', "/")
            ),
        )
        .wrap_err_with(|| format!("failed to write {curlrc}"))?;

        Ok(Self {
            http,
            identity,
            front,
            curl_home,
        })
    }

    pub(crate) fn url(&self) -> String {
        self.front.base_url()
    }

    // TODO-RAINCLAUDE: the http server's own address, for the one place
    // where plain http is the point: the target of a redirect that a script
    // must refuse.
    pub(crate) fn plain_http_url(&self) -> String {
        format!("http://{}", self.http.addr())
    }

    pub(crate) fn front_for(&self, upstream: SocketAddr) -> Result<TlsFront> {
        TlsFront::start(&self.identity, upstream)
    }

    pub(crate) fn expect(&self, expectation: Expectation) {
        self.http.expect(expectation);
    }

    pub(crate) fn verify_and_clear(&mut self) {
        self.http.verify_and_clear();
    }

    pub(crate) fn set_trust_env(&self, command: &mut Command) {
        command.env("CURL_HOME", &self.curl_home);
    }

    pub(crate) fn powershell_trust(&self, shell: PowerShellName) -> String {
        self.identity.powershell_trust(shell)
    }
}
