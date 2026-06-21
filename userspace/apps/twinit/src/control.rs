//! Unix-domain control socket server for twinit.
//!
//! Accepts short-lived client connections on `/run/twinit/control.sock`,
//! reads one text request, dispatches it through the protocol module,
//! and writes the response. Connections are not kept alive.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

use crate::protocol;
use crate::service::ServiceState;

/// A nonblocking Unix-domain socket server.
///
/// Wraps an `Option<UnixListener>` so twinit can continue operating
/// even when AF_UNIX is unavailable or the bind fails.
pub struct ControlServer {
    listener: Option<UnixListener>,
}

impl ControlServer {
    /// Bind a new control socket at `path`.
    ///
    /// Removes any stale socket file before binding. If binding fails,
    /// a warning is printed and the server is created in disabled mode
    /// so that service supervision continues unaffected.
    pub fn bind(path: &str) -> Self {
        // Remove stale socket file from a previous boot.
        let _ = fs::remove_file(path);

        let listener = match UnixListener::bind(path) {
            Ok(listener) => {
                match listener.set_nonblocking(true) {
                    Ok(()) => {
                        println!("twinit: control socket listening at {path}");
                        Some(listener)
                    }
                    Err(error) => {
                        // A blocking listener would stall PID 1 in accept(),
                        // preventing child reaping and service-log draining.
                        eprintln!(
                            "twinit: warning: cannot make control socket nonblocking: {error}"
                        );
                        drop(listener);
                        let _ = fs::remove_file(path);
                        None
                    }
                }
            }
            Err(error) => {
                eprintln!("twinit: warning: control socket unavailable: {error}");
                None
            }
        };

        Self { listener }
    }

    /// Create a no-op server that never accepts connections.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn none() -> Self {
        Self { listener: None }
    }

    /// Accept and handle all currently pending client connections.
    ///
    /// This is nonblocking: if no client is waiting, it returns
    /// immediately so the supervision loop is never stalled.
    pub fn poll_clients(&mut self, services: &[ServiceState]) {
        let Some(listener) = &self.listener else {
            return;
        };

        loop {
            let stream = match listener.accept() {
                Ok((stream, _address)) => stream,
                Err(error) => {
                    // WouldBlock (EAGAIN) means no pending connection —
                    // normal for nonblocking sockets.
                    if error.kind() != std::io::ErrorKind::WouldBlock {
                        eprintln!("twinit: control accept failed: {error}");
                    }
                    return;
                }
            };

            // Read exactly one newline-terminated request line.
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }

            let response = protocol::dispatch(&request_line, services);

            // Write the response and let the stream drop to close the
            // connection. Errors are best-effort; a broken client pipe
            // must not crash PID 1.
            let mut writer = stream;
            let _ = writer.write_all(response.as_bytes());
            let _ = writer.flush();
        }
    }
}
