//! Bootstrap socket server for twinit (XPC-like IPC broker)
//!
//! When a `CONNECT` request targets an on-demand service that is not yet
//! registered, twinit launches the service and queues the client connection.
//! Each `poll_clients` call drains pending connects that have been fulfilled
//! or have timed out, keeping PID 1 non-blocking.

use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use crate::ipc;
use crate::service::{self, ServiceState};

/// Maximum time to wait for an on-demand service to start and REGISTER before
/// replying NOT_FOUND to the waiting client.
const ON_DEMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// A client CONNECT that is waiting for an on-demand service to REGISTER.
struct PendingConnect {
    /// The client's bootstrap connection — reply goes here.
    client_stream: UnixStream,
    /// The txpc name the client requested.
    target_name: String,
    /// Client PID (from SO_PEERCRED at accept time).
    peer_pid: i32,
    /// Client UID (from SO_PEERCRED at accept time).
    peer_uid: u32,
    txpc_wire: bool,
    /// When this pending connect was created — used for timeout.
    created: Instant,
}

enum BootstrapRequest<'a> {
    Register { target_name: &'a str, txpc_wire: bool },
    Connect { target_name: &'a str, txpc_wire: bool },
}

pub struct BootstrapServer {
    listener: Option<UnixListener>,
    /// Clients waiting for an on-demand service to REGISTER.
    pending_connects: Vec<PendingConnect>,
}

impl BootstrapServer {
    /// Creates a bootstrap server with no listener and an empty pending queue.
    ///
    /// # Examples
    ///
    /// ```
    /// let server = BootstrapServer::new();
    /// ```
    pub fn new() -> Self {
        Self {
            listener: None,
            pending_connects: Vec::new(),
        }
    }

    /// Binds the bootstrap server to a Unix socket path.
    ///
    /// Any existing filesystem socket at `path` is removed before binding.
    ///
    /// # Examples
    ///
    /// ```
    /// server.bind("/tmp/twinit-bootstrap.sock");
    /// ```
    pub fn bind(&mut self, path: &str) {
    pub fn bind(&mut self, path: &str) {
        let _ = fs::remove_file(path);
        match UnixListener::bind(path) {
            Ok(listener) => {
                if let Err(err) = listener.set_nonblocking(true) {
                    eprintln!("twinit: failed to set bootstrap socket nonblocking: {err}");
                }
                self.listener = Some(listener);
                println!("twinit: listening on {path} (bootstrap)");
            }
            Err(error) => {
                eprintln!("twinit: failed to bind bootstrap socket on {path}: {error}");
            }
        }
    }

    /// Accepts queued client connections and processes pending on-demand connects.
    ///
    /// This method accepts all available connections from the bootstrap listener,
    /// handles each client request, and then revisits deferred `CONNECT` requests
    /// that are waiting for a service to register.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut server = BootstrapServer::new();
    /// let mut services: [ServiceState; 0] = [];
    /// server.poll_clients(&mut services);
    /// ```
    pub fn poll_clients(&mut self, services: &mut [ServiceState]) {
        // 1. Accept new connections into a local buffer to avoid borrowing
        //    self.listener while mutating self.pending_connects.
        let mut accepted = Vec::new();
        if let Some(listener) = &self.listener {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => accepted.push(stream),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        eprintln!("twinit: bootstrap accept failed: {error}");
                        break;
                    }
                }
            }
        }

        for mut stream in accepted {
            self.handle_client(&mut stream, services);
        }

        // 2. Drain pending on-demand connects.
        self.drain_pending(services);
    }

    /// Fulfills pending on-demand connects when their service registers or expires them after a timeout.
    
    ///
    
    /// Pending connections are removed from the queue if the target service does not register in time,
    
    /// no matching service exists, or the service exits before registering. Connections that are still
    
    /// waiting remain queued for a later poll call.
    
    ///
    
    /// # Examples
    
    ///
    
    /// ```
    
    /// # use std::time::Instant;
    
    /// # fn example() {
    
    /// #     let _ = Instant::now();
    
    /// # }
    
    /// ```
    fn drain_pending(&mut self, services: &mut [ServiceState]) {
        if self.pending_connects.is_empty() {
            return;
        }

        let now = Instant::now();

        // Take the vec so we can iterate without borrowing self.
        let pending = std::mem::take(&mut self.pending_connects);
        let mut still_pending = Vec::new();

        for mut pc in pending {
            // Check timeout first.
            if now.duration_since(pc.created) >= ON_DEMAND_TIMEOUT {
                println!(
                    "twinit: bootstrap: on-demand timeout for {} (client pid={})",
                    pc.target_name, pc.peer_pid
                );
                let _ = pc.client_stream.write_all(b"NOT_FOUND\n");
                continue; // drop this pending connect
            }

            // Check if the target service has registered.
            let target_idx = services.iter().position(|s| {
                s.config
                    .txpc
                    .as_ref()
                    .map_or(false, |txpc| txpc.name == pc.target_name)
            });

            let Some(idx) = target_idx else {
                // Service config vanished (shouldn't happen) — give up.
                let _ = pc.client_stream.write_all(b"NOT_FOUND\n");
                continue;
            };

            if services[idx].registered_txpc_fd.is_none() && services[idx].pid.is_none() {
                // The on-demand service exited or failed before REGISTER.
                let _ = pc.client_stream.write_all(b"NOT_FOUND\n");
                continue;
            }

            if services[idx].registered_txpc_fd.is_none() {
                // Still waiting — keep it queued.
                still_pending.push(pc);
                continue;
            }

            // Service is now registered — complete the connect.
            Self::complete_connect(
                &mut pc.client_stream,
                services,
                pc.peer_pid,
                pc.peer_uid,
                &pc.target_name,
                pc.txpc_wire,
            );
        }

        self.pending_connects = still_pending;
    }

    /// Handles a bootstrap request from one client connection.
    ///
    /// Reads the client's peer credentials and request payload, then dispatches
    /// the request to registration, connection, or error handling as appropriate.
    fn handle_client(&mut self, stream: &mut UnixStream, services: &mut [ServiceState]) {
        let cred = match ipc::get_peercred(stream.as_raw_fd()) {
            Ok(cred) => cred,
            Err(e) => {
                eprintln!("twinit: bootstrap: failed to get peercred: {e}");
                return;
            }
        };

        let (payload, passed_fd) = match ipc::recv_fd(stream) {
            Ok(res) => res,
            Err(e) => {
                eprintln!("twinit: bootstrap: failed to recv from client pid={}: {e}", cred.pid);
                return;
            }
        };

        match parse_bootstrap_request(&payload) {
            Some(BootstrapRequest::Register {
                target_name,
                txpc_wire,
            }) => {
                Self::handle_register(stream, services, cred.pid, target_name, passed_fd, txpc_wire);
            }
            Some(BootstrapRequest::Connect {
                target_name,
                txpc_wire,
            }) => {
                self.handle_connect(stream, services, cred.pid, cred.uid, target_name, txpc_wire);
            }
            None => {
                let _ = stream.write_all(b"ERR unknown_command\n");
                if let Some(fd) = passed_fd {
                    unsafe { crate::os::close(fd) };
                }
            }
        }
    }

    /// Registers a service's txpc socket for later connect dispatch.
    ///
    /// Requires a passed file descriptor from the service process identified by `peer_pid`.
    /// The registration succeeds only when the service exists, is configured for the named
    /// txpc endpoint, and has not already registered a txpc socket.
    ///
    /// # Parameters
    ///
    /// * `peer_pid` - PID of the service process that owns the registration request.
    /// * `target_name` - Expected txpc endpoint name for the service.
    /// * `passed_fd` - File descriptor to store as the service's txpc socket.
    /// * `txpc_wire` - Uses `TXPC_OK` when `true`, otherwise `OK`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::os::unix::net::UnixStream;
    /// # use std::os::unix::io::RawFd;
    /// # let (mut stream, _peer) = UnixStream::pair().unwrap();
    /// # let mut services = vec![];
    /// # let peer_pid = 1234;
    /// # let target_name = "example";
    /// # let passed_fd: Option<RawFd> = None;
    /// handle_register(&mut stream, &mut services, peer_pid, target_name, passed_fd, false);
    /// ```
    fn handle_register(
        stream: &mut UnixStream,
        services: &mut [ServiceState],
        peer_pid: i32,
        target_name: &str,
        passed_fd: Option<RawFd>,
        txpc_wire: bool,
    ) {
        let Some(fd) = passed_fd else {
            let _ = stream.write_all(b"ERR missing_fd\n");
            return;
        };

        let stream_to_store = unsafe { UnixStream::from_raw_fd(fd) };

        // Find the service that owns this PID
        let service = match services.iter_mut().find(|s| s.pid == Some(peer_pid)) {
            Some(s) => s,
            None => {
                let _ = stream.write_all(b"DENIED unregistered_pid\n");
                return;
            }
        };

        // Check if the service configuration has txpc, and the name matches
        let txpc = match &service.config.txpc {
            Some(txpc) => txpc,
            None => {
                let _ = stream.write_all(b"DENIED not_a_txpc_service\n");
                return;
            }
        };

        if txpc.name != target_name {
            let _ = stream.write_all(b"DENIED name_mismatch\n");
            return;
        }

        if service.registered_txpc_fd.is_some() {
            let _ = stream.write_all(b"DENIED already_registered\n");
            return;
        }

        service.registered_txpc_fd = Some(stream_to_store);
        let _ = stream.write_all(ok_response(txpc_wire).as_bytes());
    }

    /// Handles a client connect request for a txpc service.
    ///
    /// If the target service is already registered, the connection is completed immediately.
    /// If the service is marked on-demand and has not registered yet, this queues the client
    /// until the service registers or the request times out.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::os::unix::net::UnixStream;
    /// # fn demo(mut server: BootstrapServer, mut client: UnixStream, mut services: &mut [ServiceState]) {
    /// server.handle_connect(&mut client, &mut services, 1234, 1000, "my-service", false);
    /// # }
    /// ```
    ///
    /// @param client_stream The client connection used to send the response.
    /// @param services The available service states.
    /// @param peer_pid The client's process ID.
    /// @param peer_uid The client's user ID.
    /// @param target_name The requested txpc service name.
    /// @param txpc_wire Whether the request and response use the txpc wire format.
    fn handle_connect(
        &mut self,
        client_stream: &mut UnixStream,
        services: &mut [ServiceState],
        peer_pid: i32,
        peer_uid: u32,
        target_name: &str,
        txpc_wire: bool,
    ) {
        // Find the target service
        let target_service_idx = services.iter().position(|s| {
            s.config.txpc.as_ref().map_or(false, |txpc| txpc.name == target_name)
        });

        let target_idx = match target_service_idx {
            Some(idx) => idx,
            None => {
                let _ = client_stream.write_all(b"NOT_FOUND\n");
                return;
            }
        };

        let service = &services[target_idx];
        let txpc = service.config.txpc.as_ref().unwrap();

        // If not registered yet, handle on-demand launching.
        if service.registered_txpc_fd.is_none() {
            if txpc.on_demand {
                // Start the service if it isn't already running.
                if service.pid.is_none() {
                    service::start_service(&mut services[target_idx]);
                    if services[target_idx].pid.is_none() {
                        let _ = client_stream.write_all(b"NOT_FOUND\n");
                        return;
                    }
                } else {
                    println!(
                        "twinit: bootstrap: {} already running (pid={}), waiting for REGISTER",
                        target_name,
                        service.pid.unwrap()
                    );
                }

                // Clone the client stream so we can store it. try_clone()
                // duplicates the underlying fd.
                match client_stream.try_clone() {
                    Ok(cloned) => {
                        self.pending_connects.push(PendingConnect {
                            client_stream: cloned,
                            target_name: target_name.to_string(),
                            peer_pid,
                            peer_uid,
                            txpc_wire,
                            created: Instant::now(),
                        });
                    }
                    Err(e) => {
                        eprintln!("twinit: bootstrap: failed to clone client stream: {e}");
                        let _ = client_stream.write_all(b"ERR syscall_failed\n");
                    }
                }
                return;
            } else {
                let _ = client_stream.write_all(b"NOT_FOUND\n");
                return;
            }
        }

        // Service is registered — complete the connect immediately.
        Self::complete_connect(client_stream, services, peer_pid, peer_uid, target_name, txpc_wire);
    }

    /// Completes a client connection to a registered txpc service.
    ///
    /// Applies the service's access policy, creates a socket pair, and sends one
    /// end to the target service and the other to the client.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::os::unix::net::UnixStream;
    /// # fn complete_connect(
    /// #     _client_stream: &mut UnixStream,
    /// #     _services: &mut [()],
    /// #     _peer_pid: i32,
    /// #     _peer_uid: u32,
    /// #     _target_name: &str,
    /// #     _txpc_wire: bool,
    /// # ) {}
    /// # let (mut client, _peer) = UnixStream::pair().unwrap();
    /// # let mut services = [];
    /// complete_connect(&mut client, &mut services, 1234, 1000, "example", false);
    /// ```
    fn complete_connect(
        client_stream: &mut UnixStream,
        services: &mut [ServiceState],
        peer_pid: i32,
        peer_uid: u32,
        target_name: &str,
        txpc_wire: bool,
    ) {
        // Find the client service for capabilities
        let client_caps: Option<Vec<String>> = services
            .iter()
            .find(|s| s.pid == Some(peer_pid))
            .and_then(|s| s.config.txpc.as_ref())
            .and_then(|txpc| txpc.client.as_ref())
            .map(|client| client.capabilities.clone());

        let target_idx = match services.iter().position(|s| {
            s.config.txpc.as_ref().map_or(false, |txpc| txpc.name == target_name)
        }) {
            Some(idx) => idx,
            None => {
                let _ = client_stream.write_all(b"NOT_FOUND\n");
                return;
            }
        };

        let txpc = services[target_idx].config.txpc.as_ref().unwrap();

        // Apply policy
        let allowed = match &txpc.policy {
            Some(policy) => {
                if policy.allow_all {
                    true
                } else if policy.allow_uids.contains(&peer_uid) {
                    true
                } else if !policy.require_cap.is_empty() {
                    if let Some(caps) = &client_caps {
                        policy.require_cap.iter().all(|req| caps.contains(req))
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            None => false, // Default deny
        };

        if !allowed {
            let _ = client_stream.write_all(b"DENIED policy\n");
            return;
        }

        // Connection allowed: create socketpair
        let (s1, s2) = match ipc::create_socketpair() {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("twinit: bootstrap: failed to create socketpair: {e}");
                let _ = client_stream.write_all(b"ERR syscall_failed\n");
                return;
            }
        };

        let target_fd_ref = services[target_idx].registered_txpc_fd.as_ref().unwrap();

        // Send to service
        let incoming = incoming_response(txpc_wire, peer_pid);
        if let Err(e) = ipc::send_fd(target_fd_ref, &incoming, s2.as_raw_fd()) {
            eprintln!("twinit: bootstrap: failed to send fd to target {target_name}: {e}");
            let _ = client_stream.write_all(b"ERR dispatch_failed\n");
            return;
        }

        // Send to client
        if let Err(e) = ipc::send_fd(client_stream, ok_response(txpc_wire), s1.as_raw_fd()) {
            eprintln!("twinit: bootstrap: failed to send fd to client pid={peer_pid}: {e}");
            return;
        }
    }
}

/// Parses a bootstrap request from its wire format.
///
/// Supports `REGISTER <target>` and `CONNECT <target>` as well as the
/// multi-line `TXPC_REGISTER` and `TXPC_CONNECT` forms.
///
/// # Examples
///
/// ```
/// let req = parse_bootstrap_request("CONNECT my.service").unwrap();
/// match req {
///     BootstrapRequest::Connect { target_name, txpc_wire } => {
///         assert_eq!(target_name, "my.service");
///         assert!(!txpc_wire);
///     }
///     _ => unreachable!(),
/// }
/// ```
fn parse_bootstrap_request(payload: &str) -> Option<BootstrapRequest<'_>> {
    let line = payload.trim();
    if let Some(target) = line.strip_prefix("REGISTER ") {
        return Some(BootstrapRequest::Register {
            target_name: target.trim(),
            txpc_wire: false,
        });
    }
    if let Some(target) = line.strip_prefix("CONNECT ") {
        return Some(BootstrapRequest::Connect {
            target_name: target.trim(),
            txpc_wire: false,
        });
    }

    let mut lines = payload.lines();
    match lines.next()?.trim() {
        "TXPC_REGISTER" => Some(BootstrapRequest::Register {
            target_name: lines.next()?.trim(),
            txpc_wire: true,
        }),
        "TXPC_CONNECT" => Some(BootstrapRequest::Connect {
            target_name: lines.next()?.trim(),
            txpc_wire: true,
        }),
        _ => None,
    }
}

/// Builds the success response for a bootstrap request.
///
/// # Returns
///
/// `"TXPC_OK\n"` when `txpc_wire` is `true`, or `"OK\n"` otherwise.
///
/// # Examples
///
/// ```
/// assert_eq!(ok_response(false), "OK\n");
/// assert_eq!(ok_response(true), "TXPC_OK\n");
/// ```
fn ok_response(txpc_wire: bool) -> &'static str {
    if txpc_wire {
        "TXPC_OK\n"
    } else {
        "OK\n"
    }
}

/// Builds the response sent to a service for an incoming connection.
///
/// # Examples
///
/// ```
/// let msg = incoming_response(false, 1234);
/// assert_eq!(msg, "INCOMING\n");
/// ```
fn incoming_response(txpc_wire: bool, peer_pid: i32) -> String {
    if txpc_wire {
        format!("TXPC_INCOMING\n{peer_pid}\n")
    } else {
        "INCOMING\n".to_string()
    }
}
