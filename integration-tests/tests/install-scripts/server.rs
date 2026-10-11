// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    bare_tcp::{HangUpServer, StallServer},
    shells::PowerShellName,
    tls::{TlsFront, TlsIdentity},
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{Context, Result};
use httptest::{Expectation, Server};
use std::{
    any::Any,
    convert::Infallible,
    fs,
    future::Future,
    io, mem,
    net::SocketAddr,
    panic::{self, AssertUnwindSafe},
    process::Command,
};
use tokio::{
    runtime::{self, Runtime},
    task::JoinHandle,
};

struct AcceptLoopTask {
    name: String,
    handle: JoinHandle<io::Result<Infallible>>,
}

/// A test server that serves the install script, manifest, and archives.
pub(crate) struct TestServer {
    /// The runtime on which all the servers are spawned.
    runtime: Runtime,
    http: Server,
    identity: TlsIdentity,
    front: TlsFront,
    accept_loops: Vec<AcceptLoopTask>,
    // This holds a curlrc that names the test CA.
    //
    // Ordinarily, we would pass in these parameters via an environment
    // variable, but curl built for Schannel (e.g., with Git for Windows)
    // ignores CURL_CA_BUNDLE and SSL_CERT_FILE -- it does read a curlrc,
    // however.
    curl_home: Utf8PathBuf,
}

impl TestServer {
    pub(crate) fn start(dir: &Utf8Path) -> Result<Self> {
        let runtime = runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_io()
            .enable_time()
            .build()
            .wrap_err("failed to start the test servers' runtime")?;
        let http = Server::run();
        let identity = TlsIdentity::generate()?;
        let (front, front_loop) = TlsFront::bind(runtime.handle(), &identity, http.addr())?;
        let accept_loops = vec![AcceptLoopTask {
            name: "the TLS front".to_owned(),
            handle: runtime.spawn(front_loop),
        }];

        let ca_file = dir.join("test-ca.pem");
        fs::write(&ca_file, identity.ca_pem())
            .wrap_err_with(|| format!("failed to write {ca_file}"))?;
        let curl_home = dir.join("curl home");
        fs::create_dir_all(&curl_home).wrap_err_with(|| format!("failed to create {curl_home}"))?;
        let curlrc = curl_home.join(".curlrc");
        // The ssl-no-revoke config is for curl built for Schannel, which would
        // otherwise refuse the server's certificate because it doesn't mention
        // where to check for revocation. Other builds accept and ignore the
        // option, and for the purposes of this test we don't care much about
        // testing revocation anyway.
        fs::write(
            &curlrc,
            format!(
                "cacert = \"{}\"\nssl-no-revoke\n",
                ca_file.as_str().replace('\\', "/")
            ),
        )
        .wrap_err_with(|| format!("failed to write {curlrc}"))?;

        Ok(Self {
            runtime,
            http,
            identity,
            front,
            accept_loops,
            curl_home,
        })
    }

    pub(crate) fn url(&self) -> String {
        self.front.base_url()
    }

    /// Returns the underlying HTTP server's address.
    ///
    /// In most cases, [`Self::url`] should be used. This should only be used for
    /// HTTPS-to-HTTP redirection failure tests.
    pub(crate) fn plain_http_url(&self) -> String {
        format!("http://{}", self.http.addr())
    }

    pub(crate) fn front_for(&mut self, upstream: SocketAddr) -> Result<TlsFront> {
        let (front, accept) = TlsFront::bind(self.runtime.handle(), &self.identity, upstream)?;
        self.spawn_accept_loop(format!("the TLS front for {upstream}"), accept);
        Ok(front)
    }

    pub(crate) fn start_hang_up_server(&mut self) -> Result<HangUpServer> {
        let (server, accept) = HangUpServer::bind(self.runtime.handle())?;
        self.spawn_accept_loop("the hang-up server".to_owned(), accept);
        Ok(server)
    }

    pub(crate) fn start_stall_server(&mut self) -> Result<StallServer> {
        let (server, accept) = StallServer::bind(self.runtime.handle())?;
        self.spawn_accept_loop("the stall server".to_owned(), accept);
        Ok(server)
    }

    fn spawn_accept_loop(
        &mut self,
        name: String,
        accept: impl Future<Output = io::Result<Infallible>> + Send + 'static,
    ) {
        let handle = self.runtime.spawn(accept);
        self.accept_loops.push(AcceptLoopTask { name, handle });
    }

    pub(crate) fn expect(&self, expectation: Expectation) {
        self.http.expect(expectation);
    }

    pub(crate) fn take_problems(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        let verified = panic::catch_unwind(AssertUnwindSafe(|| self.http.verify_and_clear()));
        if let Err(payload) = verified {
            problems.push(format!(
                "unexpected HTTP requests: {}",
                panic_message(&*payload, "the HTTP server's expectations were not met")
            ));
        }

        let (stopped, running): (Vec<_>, Vec<_>) = mem::take(&mut self.accept_loops)
            .into_iter()
            .partition(|task| task.handle.is_finished());
        self.accept_loops = running;
        for task in stopped {
            let name = task.name;
            let problem = match self.runtime.block_on(task.handle) {
                Ok(Ok(never)) => match never {},
                Ok(Err(error)) => format!("{name} stopped accepting connections: {error}"),
                Err(error) if error.is_panic() => {
                    let payload = error.into_panic();
                    format!(
                        "{name} panicked: {}",
                        panic_message(&*payload, "(no panic message)")
                    )
                }
                Err(error) => format!("{name} was cancelled: {error}"),
            };
            problems.push(problem);
        }
        problems
    }

    pub(crate) fn set_trust_env(&self, command: &mut Command) {
        command.env("CURL_HOME", &self.curl_home);
    }

    pub(crate) fn powershell_trust(&self, shell: PowerShellName) -> String {
        self.identity.powershell_trust(shell)
    }
}

fn panic_message<'a>(payload: &'a (dyn Any + Send), fallback: &'a str) -> &'a str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or(fallback)
}
