//! Service runtime management for twinit.
//!
//! Owns `ServiceState`, process spawning, the supervision loop,
//! child reaping, restart policy enforcement, and crash-loop detection.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::bootstrap::BootstrapServer;
use crate::config::{OutputMode, RestartPolicy, ServiceConfig, ServiceType};
use crate::control::ControlServer;
use crate::os;

pub const RESTART_LIMIT: u32 = 5;
pub const RESTART_WINDOW: Duration = Duration::from_secs(10);
pub const REAP_INTERVAL: Duration = Duration::from_millis(100);
const TWILOG_SOCKET: &str = "/run/twilight/log.sock";
const LOG_READ_BUDGET: usize = 16;

// ---------------------------------------------------------------------------
// Runtime status
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum RuntimeStatus {
    Stopped,
    Starting,
    Running,
    Exited(i32),
    Failed(i32),
}

// ---------------------------------------------------------------------------
// Service state
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ServiceState {
    pub config: ServiceConfig,
    pub pid: Option<i32>,
    pub status: RuntimeStatus,
    pub restart_count: u32,
    pub restart_window_started: Instant,
    pub disabled: bool,
    pub stdout_log: Option<File>,
    pub stderr_log: Option<File>,
    pub stdout_buffer: String,
    pub stderr_buffer: String,
    pub registered_txpc_fd: Option<UnixStream>,
}

impl ServiceState {
    /// Creates a new service runtime state for the given configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// let state = ServiceState::new(config);
    /// assert!(state.pid.is_none());
    /// assert!(matches!(state.status, RuntimeStatus::Stopped));
    /// ```
    pub fn new(config: ServiceConfig) -> Self {
        Self {
            config,
            pid: None,
            status: RuntimeStatus::Stopped,
            restart_count: 0,
            restart_window_started: Instant::now(),
            disabled: false,
            stdout_log: None,
            stderr_log: None,
            stdout_buffer: String::new(),
            stderr_buffer: String::new(),
            registered_txpc_fd: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Process spawning
// ---------------------------------------------------------------------------

/// Starts a service process and updates its runtime state.
///
/// If the service is disabled or already running, this does nothing.
///
/// # Examples
///
/// ```ignore
/// start_service(&mut service);
/// ```
pub fn start_service(service: &mut ServiceState) {
    if service.disabled || service.pid.is_some() {
        return;
    }
    service.status = RuntimeStatus::Starting;
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();

    let stdout_pipe = prepare_log_pipe(&service.config.name, "stdout", service.config.stdout);
    let stderr_pipe = prepare_log_pipe(&service.config.name, "stderr", service.config.stderr);

    match os::fork_process() {
        Ok(0) => {
            let stdout_writer = child_pipe_writer(stdout_pipe);
            let stderr_writer = child_pipe_writer(stderr_pipe);
            run_service_child(&service.config, stdout_writer, stderr_writer)
        }
        Ok(pid) => {
            service.stdout_log = parent_pipe_reader(stdout_pipe);
            service.stderr_log = parent_pipe_reader(stderr_pipe);
            service.stdout_buffer.clear();
            service.stderr_buffer.clear();
            service.pid = Some(pid);
            service.status = RuntimeStatus::Running;
        }
        Err(error) => {
            service.status = RuntimeStatus::Failed(-1);
            eprintln!(
                "twinit: failed to fork service {}: {error}",
                service.config.name
            );
        }
    }
}

fn prepare_log_pipe(
    service_name: &str,
    stream_name: &str,
    mode: OutputMode,
) -> Option<(File, File)> {
    if mode != OutputMode::Log {
        return None;
    }
    match os::create_log_pipe() {
        Ok(pipe) => Some(pipe),
        Err(error) => {
            eprintln!("twinit: cannot create {stream_name} log pipe for {service_name}: {error}");
            None
        }
    }
}

fn child_pipe_writer(pipe: Option<(File, File)>) -> Option<File> {
    pipe.map(|(reader, writer)| {
        drop(reader);
        writer
    })
}

fn parent_pipe_reader(pipe: Option<(File, File)>) -> Option<File> {
    pipe.map(|(reader, writer)| {
        drop(writer);
        reader
    })
}

fn run_service_child(
    config: &ServiceConfig,
    stdout_log: Option<File>,
    stderr_log: Option<File>,
) -> ! {
    match config.service_type {
        ServiceType::Foreground => {}
    }
    if let Err(error) = os::create_session() {
        eprintln!("twinit: service {} setsid failed: {error}", config.name);
    }

    // Console services keep fd 0 so an interactive foreground shell remains
    // usable. Non-console services receive /dev/null as stdin.
    if config.stdout != OutputMode::Console || config.stderr != OutputMode::Console {
        if let Ok(stdin) = File::open("/dev/null") {
            let _ = os::duplicate_fd(stdin.as_raw_fd(), 0);
        }
    }
    redirect_output(config.stdout, 1, stdout_log.as_ref());
    redirect_output(config.stderr, 2, stderr_log.as_ref());
    drop(stdout_log);
    drop(stderr_log);

    let mut command = Command::new(&config.exec);
    command.args(&config.args);
    command.env("PATH", "/sbin:/bin:/usr/sbin:/usr/bin");
    command.env("TWILIGHT_SERVICE", &config.name);
    let error = command.exec();
    eprintln!(
        "twinit: exec failed for service {} ({}): {error}",
        config.name, config.exec
    );
    os::exit_child(127)
}

fn redirect_output(mode: OutputMode, target_fd: i32, log_writer: Option<&File>) {
    match mode {
        OutputMode::Console => {}
        OutputMode::Null => {
            let null = OpenOptions::new().write(true).open("/dev/null");
            match null {
                Ok(file) => {
                    if let Err(error) = os::duplicate_fd(file.as_raw_fd(), target_fd) {
                        eprintln!("twinit: dup2({target_fd}) failed: {error}");
                    }
                }
                Err(error) => eprintln!("twinit: cannot open /dev/null: {error}"),
            }
        }
        OutputMode::Log => match log_writer {
            Some(writer) => {
                if let Err(error) = os::duplicate_fd(writer.as_raw_fd(), target_fd) {
                    eprintln!("twinit: log dup2({target_fd}) failed: {error}");
                }
            }
            None => eprintln!("twinit: log pipe unavailable for fd {target_fd}; using console"),
        },
    }
}

// ---------------------------------------------------------------------------
// Supervision loop
// ---------------------------------------------------------------------------

/// Runs the service supervision loop.
///
/// The loop reaps exited children, processes control and bootstrap clients, drains service logs,
/// and pauses briefly between iterations.
///
/// # Examples
///
/// ```ignore
/// let mut services: Vec<ServiceState> = Vec::new();
/// let mut control: ControlServer = todo!();
/// let mut bootstrap: BootstrapServer = todo!();
/// supervise(&mut services, &mut control, &mut bootstrap);
/// ```
pub fn supervise(
    services: &mut [ServiceState],
    control: &mut ControlServer,
    bootstrap: &mut BootstrapServer,
) -> ! {
    loop {
        // 1. Reap all exited children.
        loop {
            match os::reap_one() {
                Ok(Some((pid, status))) => handle_child_exit(services, pid, status),
                Ok(None) => break,
                Err(error) if error.raw_os_error() == Some(10) => break, // ECHILD
                Err(error) => {
                    eprintln!("twinit: waitpid failed: {error}");
                    break;
                }
            }
        }

        // 2. Accept and process any pending control socket clients.
        control.poll_clients(services);

        // 3. Accept and process any pending bootstrap socket clients.
        bootstrap.poll_clients(services);

        // 4. Drain bounded chunks from service stdout/stderr log pipes.
        drain_service_logs(services);

        // 5. Sleep briefly before the next iteration.
        os::sleep(REAP_INTERVAL);
    }
}

fn drain_service_logs(services: &mut [ServiceState]) {
    for service in services {
        drain_log_pipe(
            &service.config.name,
            "INFO",
            &mut service.stdout_log,
            &mut service.stdout_buffer,
        );
        drain_log_pipe(
            &service.config.name,
            "ERROR",
            &mut service.stderr_log,
            &mut service.stderr_buffer,
        );
    }
}

fn drain_log_pipe(service_name: &str, level: &str, pipe: &mut Option<File>, pending: &mut String) {
    let Some(reader) = pipe.as_mut() else {
        return;
    };

    let mut reached_eof = false;
    let mut chunk = [0_u8; 1024];
    for _ in 0..LOG_READ_BUDGET {
        match reader.read(&mut chunk) {
            Ok(0) => {
                reached_eof = true;
                break;
            }
            Ok(read) => {
                pending.push_str(&String::from_utf8_lossy(&chunk[..read]));
                forward_complete_lines(service_name, level, pending);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("twinit: log pipe read failed source={service_name}: {error}");
                reached_eof = true;
                break;
            }
        }
    }

    if reached_eof {
        if !pending.is_empty() {
            let line = pending.trim_end_matches('\r').to_string();
            pending.clear();
            if !line.is_empty() {
                forward_service_log(service_name, level, &line);
            }
        }
        *pipe = None;
    }
}

fn forward_complete_lines(service_name: &str, level: &str, pending: &mut String) {
    while let Some(newline) = pending.find('\n') {
        let line = pending[..newline].trim_end_matches('\r').to_string();
        pending.drain(..=newline);
        if !line.is_empty() {
            forward_service_log(service_name, level, &line);
        }
    }
}

pub fn forward_service_log(service_name: &str, level: &str, line: &str) {
    let clean_line = line.replace(['\r', '\n'], " ");
    let request = format!("LEVEL={level} SOURCE={service_name} MESSAGE={clean_line}");
    let result = UnixDatagram::unbound()
        .and_then(|socket| socket.send_to(request.as_bytes(), TWILOG_SOCKET));
    if let Err(error) = result {
        println!("twinit: log fallback source={service_name} message={clean_line} error={error}");
    }
}

// ---------------------------------------------------------------------------
// Child exit handling
// ---------------------------------------------------------------------------

/// Updates a service after a child process exits and applies restart policy.
///
/// Clears the running process association, drains any remaining log output, updates the
/// recorded runtime status from the wait status, and restarts the service when its
/// policy allows it. If repeated exits exceed the restart limit within the restart
/// window, the service is disabled.
///
/// # Examples
///
/// ```
/// # use std::time::Instant;
/// # fn handle_child_exit(_: &mut [ServiceState], _: i32, _: i32) {}
/// ```
fn handle_child_exit(services: &mut [ServiceState], pid: i32, wait_status: i32) {
    let Some(service) = services.iter_mut().find(|service| service.pid == Some(pid)) else {
        println!("twinit: reaped unknown child pid={pid} status={wait_status:#x}");
        return;
    };

    service.pid = None;
    service.registered_txpc_fd = None;
    drain_log_pipe(
        &service.config.name,
        "INFO",
        &mut service.stdout_log,
        &mut service.stdout_buffer,
    );
    drain_log_pipe(
        &service.config.name,
        "ERROR",
        &mut service.stderr_log,
        &mut service.stderr_buffer,
    );
    let outcome = decode_wait_status(wait_status);
    match outcome {
        WaitOutcome::Exited(code) => {
            service.status = if code == 0 {
                RuntimeStatus::Exited(code)
            } else {
                RuntimeStatus::Failed(code)
            };
        }
        WaitOutcome::Signaled(signal) => {
            println!(
                "twinit: service {} pid={pid} exited signal={signal}",
                service.config.name
            );
            service.status = RuntimeStatus::Failed(128 + signal);
        }
        WaitOutcome::Other(raw) => {
            println!(
                "twinit: service {} pid={pid} changed state status={raw:#x}",
                service.config.name
            );
            service.status = RuntimeStatus::Failed(raw);
        }
    }

    if !restart_required(service.config.restart, outcome) {
        return;
    }

    let now = Instant::now();
    if !record_restart_attempt(service, now) {
        service.disabled = true;
        println!(
            "twinit: service {} disabled after crash loop",
            service.config.name
        );
        return;
    }

    println!(
        "twinit: restarting service {} policy={}",
        service.config.name,
        service.config.restart.as_str()
    );
    start_service(service);
}

// ---------------------------------------------------------------------------
// Restart logic
// ---------------------------------------------------------------------------

fn restart_required(policy: RestartPolicy, outcome: WaitOutcome) -> bool {
    match policy {
        RestartPolicy::Never => false,
        RestartPolicy::OnFailure => !matches!(outcome, WaitOutcome::Exited(0)),
        RestartPolicy::Always => true,
    }
}

fn record_restart_attempt(service: &mut ServiceState, now: Instant) -> bool {
    if now.duration_since(service.restart_window_started) > RESTART_WINDOW {
        service.restart_window_started = now;
        service.restart_count = 0;
    }
    if service.restart_count >= RESTART_LIMIT {
        return false;
    }
    service.restart_count += 1;
    true
}

// ---------------------------------------------------------------------------
// Wait status decoding
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum WaitOutcome {
    Exited(i32),
    Signaled(i32),
    Other(i32),
}

fn decode_wait_status(status: i32) -> WaitOutcome {
    let signal = status & 0x7f;
    if signal == 0 {
        WaitOutcome::Exited((status >> 8) & 0xff)
    } else if signal != 0x7f {
        WaitOutcome::Signaled(signal)
    } else {
        WaitOutcome::Other(status)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_RUNLEVEL, fallback_shell};

    #[test]
    fn decodes_linux_wait_status() {
        assert!(matches!(
            decode_wait_status(42 << 8),
            WaitOutcome::Exited(42)
        ));
        assert!(matches!(decode_wait_status(9), WaitOutcome::Signaled(9)));
    }

    #[test]
    fn applies_restart_policies() {
        assert!(!restart_required(
            RestartPolicy::Never,
            WaitOutcome::Exited(1)
        ));
        assert!(!restart_required(
            RestartPolicy::OnFailure,
            WaitOutcome::Exited(0)
        ));
        assert!(restart_required(
            RestartPolicy::OnFailure,
            WaitOutcome::Exited(1)
        ));
        assert!(restart_required(
            RestartPolicy::OnFailure,
            WaitOutcome::Signaled(9)
        ));
        assert!(restart_required(
            RestartPolicy::Always,
            WaitOutcome::Exited(0)
        ));
    }

    #[test]
    fn limits_fast_restart_loops() {
        let mut service = ServiceState::new(fallback_shell(DEFAULT_RUNLEVEL));
        let now = service.restart_window_started;
        for _ in 0..RESTART_LIMIT {
            assert!(record_restart_attempt(&mut service, now));
        }
        assert!(!record_restart_attempt(&mut service, now));

        let later = now + RESTART_WINDOW + Duration::from_secs(1);
        assert!(record_restart_attempt(&mut service, later));
        assert_eq!(service.restart_count, 1);
    }
}
