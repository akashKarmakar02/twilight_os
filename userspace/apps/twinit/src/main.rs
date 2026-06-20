//! twinit — Twilight OS PID 1 service manager.
//!
//! Loads service configurations, spawns supervised processes, reaps
//! children, and listens for read-only control queries from `twinitctl`
//! over a Unix-domain socket.

mod config;
mod control;
mod os;
mod protocol;
mod service;

use std::env;
use std::fs;
use std::io;
use std::path::Path;

use config::{DEFAULT_RUNLEVEL, SERVICE_DIR, fallback_shell, load_service_configs};
use control::ControlServer;
use service::{ServiceState, start_service, supervise};

const TWINIT_RUNTIME_DIR: &str = "/run/twinit";
const TWINIT_CONTROL_SOCK: &str = "/run/twinit/control.sock";

fn main() {
    let pid = std::process::id();
    if pid != 1 {
        println!("twinit: warning: not running as pid 1");
    }

    ensure_runtime_directory();

    let mut runlevel = DEFAULT_RUNLEVEL;
    if let Some(command) = env::args_os().nth(1) {
        match command.to_string_lossy().as_ref() {
            "--shutdown" => {
                command_hook("shutdown", pid);
                return;
            }
            "--reboot" => {
                command_hook("reboot", pid);
                return;
            }
            "--single" if pid == 1 => runlevel = "single",
            "--single" => {
                println!("twinit: would enter single-user runlevel");
                return;
            }
            other => {
                eprintln!("twinit: unknown option: {other}");
                eprintln!("usage: twinit [--shutdown|--reboot|--single]");
                return;
            }
        }
    }

    let mut control = ControlServer::bind(TWINIT_CONTROL_SOCK);

    let mut configs = load_service_configs(Path::new(SERVICE_DIR), runlevel);
    // Logging is an early infrastructure service. This is intentionally a
    // single bootstrap rule, not a dependency solver.
    configs.sort_by_key(|config| config.name != "twilogd");
    let mut services = if configs.is_empty() {
        vec![ServiceState::new(fallback_shell(runlevel))]
    } else {
        configs.into_iter().map(ServiceState::new).collect()
    };

    for service in &mut services {
        start_service(service);
    }

    supervise(&mut services, &mut control);
}

fn ensure_runtime_directory() {
    for directory in ["/run", TWINIT_RUNTIME_DIR] {
        match fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                eprintln!("twinit: cannot create runtime directory {directory}: {error}");
                return;
            }
        }
    }
}

fn command_hook(action: &str, pid: u32) {
    if pid != 1 {
        println!("twinit: would request system {action}");
        return;
    }
    println!("twinit: TODO: orderly {action} is not implemented");
}
