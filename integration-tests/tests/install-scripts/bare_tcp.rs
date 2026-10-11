// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use color_eyre::eyre::{Context, Result};
use std::{
    io::{BufRead, BufReader},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// A bare TCP server, to test scenarios that an HTTP library would not produce.
///
/// The server reads each request's head, then hands the connection to
/// `on_request`.
struct AcceptLoop {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AcceptLoop {
    fn start(mut on_request: impl FnMut(TcpStream) + Send + 'static) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .wrap_err("failed to bind the test server")?;
        let addr = listener
            .local_addr()
            .wrap_err("failed to get the test server's address")?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::spawn({
            let stop = stop.clone();
            move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    read_request_head(&stream);
                    on_request(stream);
                }
            }
        });
        Ok(Self {
            addr,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for AcceptLoop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so that it sees the stop flag.
        let woken = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            // A thread that was not woken never leaves accept, so joining it
            // would cause the test to hang. Leave it detached and allow for
            // process cleanup to tear down the thread.
            if woken.is_ok() {
                let _ = thread.join();
            }
        }
    }
}

pub(crate) struct HangUpServer {
    accept: AcceptLoop,
    hang_ups: mpsc::Receiver<()>,
}

impl HangUpServer {
    pub(crate) fn start() -> Result<Self> {
        let (tx, hang_ups) = mpsc::channel();
        let accept = AcceptLoop::start(move |stream| {
            // Send a message before dropping -- the client only sees the
            // hang-up once the stream is gone, so the count is there by
            // the time the client notices.
            let _ = tx.send(());
            drop(stream);
        })?;
        Ok(Self { accept, hang_ups })
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.accept.addr
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
    accept: AcceptLoop,
    stalled: mpsc::Receiver<TcpStream>,
}

impl StallServer {
    pub(crate) fn start() -> Result<Self> {
        let (tx, stalled) = mpsc::channel();
        let accept = AcceptLoop::start(move |stream| {
            let _ = tx.send(stream);
        })?;
        Ok(Self { accept, stalled })
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.accept.addr
    }

    /// Returns the next stalled request, or an error if no request was received
    /// within 60 seconds.
    pub(crate) fn wait_for_request(&self) -> Result<TcpStream> {
        // The timeout is only required for a failing test (e.g., a script that
        // exits without making a request). Passing tests do not hit it.
        self.stalled
            .recv_timeout(Duration::from_secs(60))
            .wrap_err("the script did not make the stalled request within 60 seconds")
    }
}

/// Reads up to the blank line that ends the headers.
///
/// This ensures that the client has finished sending what it can before the
/// connection is closed.
fn read_request_head(stream: &TcpStream) {
    if stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .is_err()
    {
        return;
    }
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if line == "\r\n" || line == "\n" => return,
            Ok(_) => {}
        }
    }
}
