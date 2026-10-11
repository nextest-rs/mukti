// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::shells::PowerShellName;
#[cfg(windows)]
use byte_wrapper::Base64Vec;
use color_eyre::eyre::{Context, Result};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    crypto::aws_lc_rs,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    ServerConfig,
};
use std::{
    convert::Infallible,
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr},
    panic,
    sync::Arc,
};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    runtime::Handle,
    task::JoinSet,
};
use tokio_rustls::TlsAcceptor;

/// A throwaway certificate authority, and a certificate it issued for
/// 127.0.0.1.
///
/// Used for testing TLS.
pub(crate) struct TlsIdentity {
    acceptor: TlsAcceptor,
    ca_pem: String,
    // The server certificate itself.
    #[cfg(windows)]
    certificate_der: Base64Vec,
}

impl TlsIdentity {
    pub(crate) fn generate() -> Result<Self> {
        let ca_key = KeyPair::generate().wrap_err("failed to generate the test CA's key")?;
        let mut ca_params = CertificateParams::new(Vec::new())
            .wrap_err("failed to describe the test CA's certificate")?;
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "mukti test CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca_certificate = ca_params
            .self_signed(&ca_key)
            .wrap_err("failed to sign the test CA's certificate")?;
        let issuer = Issuer::from_params(&ca_params, &ca_key);

        let key = KeyPair::generate().wrap_err("failed to generate the test server's key")?;
        let mut params = CertificateParams::new(vec![Ipv4Addr::LOCALHOST.to_string()])
            .wrap_err("failed to describe the test server's certificate")?;
        params
            .distinguished_name
            .push(DnType::CommonName, "mukti test server");
        let certificate = params
            .signed_by(&key, &issuer)
            .wrap_err("failed to sign the test server's certificate")?;

        let config = ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .wrap_err("failed to choose TLS versions for the test server")?
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .wrap_err("failed to configure the test server's certificate")?;

        Ok(Self {
            acceptor: TlsAcceptor::from(Arc::new(config)),
            ca_pem: ca_certificate.pem(),
            #[cfg(windows)]
            certificate_der: Base64Vec::new(certificate.der().to_vec()),
        })
    }

    pub(crate) fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    /// Returns the PowerShell commands to run in order to trust the test
    /// certificate.
    ///
    /// This is careful not to make any system-level changes.
    pub(crate) fn powershell_trust(&self, shell: PowerShellName) -> String {
        match shell {
            PowerShellName::Pwsh => {
                // With PowerShell 7, the web cmdlets use HttpClient, which
                // ignores ServicePointManager's cert validation callback. The
                // only real option is to ignore cert checks entirely. (A
                // different option is to install our test cert in the system
                // store, which seems like a horrible idea.)
                "$PSDefaultParameterValues['Invoke-WebRequest:SkipCertificateCheck'] = $true; \
                 $PSDefaultParameterValues['Invoke-RestMethod:SkipCertificateCheck'] = $true; "
                    .to_owned()
            }
            // Explicitly import a couple of modules to speed up script runtime.
            // (Without the explicit imports, the tests were observed to take
            // up to 50s on the GitHub runners.)
            #[cfg(windows)]
            PowerShellName::WindowsPowerShell => format!(
                "Import-Module Microsoft.PowerShell.Utility, Microsoft.PowerShell.Management; \
                 Add-Type -TypeDefinition '{TRUST_CALLBACK}'; \
                 [MuktiTestTrust]::Expected = '{}'; [MuktiTestTrust]::Install(); ",
                self.certificate_der
            ),
        }
    }
}

pub(crate) struct TlsFront {
    addr: SocketAddr,
}

impl TlsFront {
    pub(crate) fn bind(
        runtime: &Handle,
        identity: &TlsIdentity,
        upstream: SocketAddr,
    ) -> Result<(
        Self,
        impl Future<Output = io::Result<Infallible>> + Send + 'static,
    )> {
        let acceptor = identity.acceptor.clone();
        let listener = runtime
            .block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))
            .wrap_err("failed to bind the TLS front")?;
        let addr = listener
            .local_addr()
            .wrap_err("failed to read the TLS front's address")?;
        Ok((Self { addr }, tls_accept_loop(listener, acceptor, upstream)))
    }

    pub(crate) fn base_url(&self) -> String {
        format!("https://{}", self.addr)
    }
}

async fn tls_accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    upstream: SocketAddr,
) -> io::Result<Infallible> {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                connections.spawn(proxy_connection(acceptor.clone(), stream, upstream));
            }
            Some(joined) = connections.join_next() => match joined {
                Ok(()) => {}
                Err(error) if error.is_panic() => panic::resume_unwind(error.into_panic()),
                // A non-panic JoinError means cancellation, which only happens
                // at runtime shutdown (we never cancel tasks ourselves at the moment).
                Err(_) => {}
            },
        }
    }
}

async fn proxy_connection(acceptor: TlsAcceptor, stream: TcpStream, upstream: SocketAddr) {
    // Errors here are caused by either the client (for example,
    // refusing the certificate) or the upstream server
    // (hanging up on purpose). In either case we (probably?)
    // don't have to handle or log the error here.
    let Ok(mut tls) = acceptor.accept(stream).await else {
        return;
    };
    let Ok(mut plain) = TcpStream::connect(upstream).await else {
        return;
    };
    let _ = copy_bidirectional(&mut tls, &mut plain).await;
}

/// An installed callback for trusting a test certificate.
///
/// This is C# without any quote characters so that it can live inside a
/// single-quoted PowerShell string. Note that the callback cannot be a script
/// block, because .NET calls it on a thread without a PowerShell runspace.
#[cfg(windows)]
const TRUST_CALLBACK: &str = "using System.Net; using System.Net.Security; \
    using System.Security.Cryptography.X509Certificates; \
    public static class MuktiTestTrust { \
    public static string Expected; \
    public static void Install() { ServicePointManager.ServerCertificateValidationCallback = Check; } \
    static bool Check(object sender, X509Certificate certificate, X509Chain chain, SslPolicyErrors errors) { \
    return certificate != null && System.Convert.ToBase64String(certificate.GetRawCertData()) == Expected; } }";
