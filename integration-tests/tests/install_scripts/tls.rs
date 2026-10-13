// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::shells::ShellName;
use color_eyre::eyre::{bail, Context, Result};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{
    crypto::aws_lc_rs,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    ServerConfig,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    runtime::{self, Runtime},
};
use tokio_rustls::TlsAcceptor;

// TODO-RAINCLAUDE: a throwaway certificate authority and a certificate it
// issued for 127.0.0.1. Nothing trusts either unless a test tells it to. One
// identity serves every front of a fixture, so that a client told about it
// accepts them all.
pub(crate) struct TlsIdentity {
    acceptor: TlsAcceptor,
    ca_pem: String,
    // TODO-RAINCLAUDE: the server certificate itself (DER, in base64).
    #[cfg(windows)]
    certificate_base64: String,
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
            certificate_base64: certificate
                .pem()
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect(),
        })
    }

    pub(crate) fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    // TODO-RAINCLAUDE: PowerShell statements, to run before anything that
    // downloads, that make irm and the script's own downloads accept this
    // identity. No certificate store is touched. PowerShell 7 can only be
    // told to skip the check; Windows PowerShell is told to accept exactly
    // this certificate.
    pub(crate) fn powershell_trust(&self, shell: ShellName) -> Result<String> {
        match shell {
            ShellName::Pwsh => Ok(
                "$PSDefaultParameterValues['Invoke-WebRequest:SkipCertificateCheck'] = $true; \
                 $PSDefaultParameterValues['Invoke-RestMethod:SkipCertificateCheck'] = $true; "
                    .to_owned(),
            ),
            // TODO-RAINCLAUDE: these statements run before the script, so
            // the script's own import of its modules comes too late for
            // them. Import by name here too, for the reason given at that
            // import in install.ps1: Utility for Add-Type and irm, and
            // Management for the Set-Location that the piped and script
            // block launches run next. Without Management, those launches
            // still took 30 to 49 s each on the GitHub runners.
            #[cfg(windows)]
            ShellName::WindowsPowerShell => Ok(format!(
                "Import-Module Microsoft.PowerShell.Utility, Microsoft.PowerShell.Management; \
                 Add-Type -TypeDefinition '{TRUST_CALLBACK}'; \
                 [MuktiTestTrust]::Expected = '{}'; [MuktiTestTrust]::Install(); ",
                self.certificate_base64
            )),
            ShellName::Sh
            | ShellName::Dash
            | ShellName::Bash
            | ShellName::Busybox
            | ShellName::Ksh93 => {
                bail!("{} is not a PowerShell", shell.as_str());
            }
        }
    }
}

// TODO-RAINCLAUDE: an https address for a plain server: accepts TLS
// connections on 127.0.0.1 and passes the decrypted bytes on, in both
// directions.
pub(crate) struct TlsFront {
    // TODO-RAINCLAUDE: runs the accept loop; dropping it stops the front.
    _runtime: Runtime,
    addr: SocketAddr,
}

impl TlsFront {
    pub(crate) fn start(identity: &TlsIdentity, upstream: SocketAddr) -> Result<Self> {
        let acceptor = identity.acceptor.clone();
        let runtime = runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_io()
            .build()
            .wrap_err("failed to start a runtime for the TLS front")?;
        let listener = runtime
            .block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))
            .wrap_err("failed to bind the TLS front")?;
        let addr = listener
            .local_addr()
            .wrap_err("failed to read the TLS front's address")?;
        runtime.spawn(async move {
            // TODO-RAINCLAUDE: a failed accept ends the front; the test then
            // fails on its missing requests.
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    // TODO-RAINCLAUDE: errors here are the client's doing (for
                    // example, refusing the certificate) or the upstream
                    // server's (hanging up on purpose), and show up in the
                    // script's own output.
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let Ok(mut plain) = TcpStream::connect(upstream).await else {
                        return;
                    };
                    let _ = copy_bidirectional(&mut tls, &mut plain).await;
                });
            }
        });

        Ok(Self {
            _runtime: runtime,
            addr,
        })
    }

    pub(crate) fn base_url(&self) -> String {
        format!("https://{}", self.addr)
    }
}

// TODO-RAINCLAUDE: C# with no quote characters, so that it can sit inside a
// single-quoted PowerShell string. A script block cannot be the callback:
// .NET calls it on a thread with no PowerShell runspace.
#[cfg(windows)]
const TRUST_CALLBACK: &str = "using System.Net; using System.Net.Security; \
    using System.Security.Cryptography.X509Certificates; \
    public static class MuktiTestTrust { \
    public static string Expected; \
    public static void Install() { ServicePointManager.ServerCertificateValidationCallback = Check; } \
    static bool Check(object sender, X509Certificate certificate, X509Chain chain, SslPolicyErrors errors) { \
    return certificate != null && System.Convert.ToBase64String(certificate.GetRawCertData()) == Expected; } }";
