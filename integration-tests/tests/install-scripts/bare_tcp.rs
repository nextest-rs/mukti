// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use color_eyre::eyre::{Context, Result};
use std::{
    convert::Infallible,
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::mpsc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::{TcpListener, TcpStream},
    runtime::Handle,
    time::timeout,
};

/// A bare TCP server, to test scenarios that an HTTP library would not produce.
///
/// The server reads each request's head, then hands the connection to
/// `on_request`.
fn bind_accept_loop(
    runtime: &Handle,
    on_request: impl FnMut(TcpStream) + Send + 'static,
) -> Result<(
    SocketAddr,
    impl Future<Output = io::Result<Infallible>> + Send + 'static,
)> {
    let listener = runtime
        .block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))
        .wrap_err("failed to bind the test server")?;
    let addr = listener
        .local_addr()
        .wrap_err("failed to get the test server's address")?;
    Ok((addr, accept_loop(listener, on_request)))
}

async fn accept_loop(
    listener: TcpListener,
    mut on_request: impl FnMut(TcpStream),
) -> io::Result<Infallible> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        read_request_head(&mut stream).await;
        on_request(stream);
    }
}

pub(crate) struct HangUpServer {
    addr: SocketAddr,
    hang_ups: mpsc::Receiver<()>,
}

impl HangUpServer {
    pub(crate) fn bind(
        runtime: &Handle,
    ) -> Result<(
        Self,
        impl Future<Output = io::Result<Infallible>> + Send + 'static,
    )> {
        let (tx, hang_ups) = mpsc::channel();
        let (addr, accept) = bind_accept_loop(runtime, move |stream| {
            // Send a message before dropping -- the client only sees the
            // hang-up once the stream is gone, so the count is there by
            // the time the client notices.
            let _ = tx.send(());
            drop(stream);
        })?;
        Ok((Self { addr, hang_ups }, accept))
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The number of connections dropped since the last call.
    ///
    /// Calling this drains the channel.
    pub(crate) fn take_hang_up_count(&self) -> usize {
        self.hang_ups.try_iter().count()
    }
}

/// Holds each connection open without responding.
///
/// The next stalled connection is accessible via [`Self::wait_for_request`].
pub(crate) struct StallServer {
    addr: SocketAddr,
    stalled: mpsc::Receiver<TcpStream>,
}

impl StallServer {
    pub(crate) fn bind(
        runtime: &Handle,
    ) -> Result<(
        Self,
        impl Future<Output = io::Result<Infallible>> + Send + 'static,
    )> {
        let (tx, stalled) = mpsc::channel();
        let (addr, accept) = bind_accept_loop(runtime, move |stream| {
            let _ = tx.send(stream);
        })?;
        Ok((Self { addr, stalled }, accept))
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Drains the stalled channel, closing those connections, and returns the
    /// number of stalled requests.
    pub(crate) fn take_stalled_count(&self) -> usize {
        self.stalled.try_iter().count()
    }

    /// Returns the next stalled request, or an error if no request was received
    /// within 60 seconds.
    pub(crate) fn wait_for_request(&self) -> Result<std::net::TcpStream> {
        // The timeout is only required for a failing test (e.g., a script that
        // exits without making a request). Passing tests do not hit it.
        self.stalled
            .recv_timeout(Duration::from_secs(60))
            .wrap_err("the script did not make the stalled request within 60 seconds")
            .map(|stream| {
                stream
                    .into_std()
                    .expect("stalled connection converted to a std stream")
            })
    }
}

/// Reads up to the blank line that ends the headers.
///
/// This ensures that the client has finished sending what it can before the
/// connection is closed.
async fn read_request_head(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream);
    let read_head = async {
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) if line == "\r\n" || line == "\n" => return,
                Ok(_) => {}
            }
        }
    };
    let _ = timeout(Duration::from_secs(30), read_head).await;
}
