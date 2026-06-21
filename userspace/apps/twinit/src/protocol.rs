//! Text-based control protocol for twinit.
//!
//! Parses single-line requests from `twinitctl`, dispatches them against
//! the service table, and produces line-based text responses.

use crate::service::{RuntimeStatus, ServiceState};

// ---------------------------------------------------------------------------
// Request parsing
// ---------------------------------------------------------------------------

enum Request<'a> {
    Status,
    List,
    Show(&'a str),
    Ping,
    Help,
    Unknown,
}

fn parse_request(line: &str) -> Request<'_> {
    let line = line.trim();
    if line.eq_ignore_ascii_case("STATUS") {
        Request::Status
    } else if line.eq_ignore_ascii_case("LIST") {
        Request::List
    } else if line.eq_ignore_ascii_case("PING") {
        Request::Ping
    } else if line.eq_ignore_ascii_case("HELP") {
        Request::Help
    } else if let Some(name) = line
        .strip_prefix("SHOW ")
        .or_else(|| line.strip_prefix("show "))
    {
        let name = name.trim();
        if name.is_empty() {
            Request::Unknown
        } else {
            Request::Show(name)
        }
    } else {
        Request::Unknown
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Process a raw request line and return the full text response.
pub fn dispatch(request_line: &str, services: &[ServiceState]) -> String {
    match parse_request(request_line) {
        Request::Status | Request::List => {
            let body = format_all_services(services);
            if body.is_empty() {
                "OK\n".to_string()
            } else {
                format!("OK\n{body}")
            }
        }
        Request::Show(name) => match find_service(services, name) {
            Some(service) => {
                let line = format_service_status(service);
                format!("OK\n{line}\n")
            }
            None => "ERR service not found\n".to_string(),
        },
        Request::Ping => "OK pong\n".to_string(),
        Request::Help => concat!(
            "OK\n",
            "STATUS  - show all services\n",
            "LIST    - list all services\n",
            "SHOW N  - show service N\n",
            "PING    - health check\n",
            "HELP    - this message\n",
        )
        .to_string(),
        Request::Unknown => "ERR unsupported command\n".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Service formatting
// ---------------------------------------------------------------------------

pub fn runtime_status_name(status: RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Stopped => "stopped",
        RuntimeStatus::Starting => "starting",
        RuntimeStatus::Running => "running",
        RuntimeStatus::Exited(_) => "exited",
        RuntimeStatus::Failed(_) => "failed",
    }
}

pub fn format_service_status(service: &ServiceState) -> String {
    let pid_str = match service.pid {
        Some(pid) => pid.to_string(),
        None => "-1".to_string(),
    };

    let state = runtime_status_name(service.status);

    let code_part = match service.status {
        RuntimeStatus::Exited(code) => format!(" code={code}"),
        RuntimeStatus::Failed(code) => format!(" code={code}"),
        _ => String::new(),
    };

    format!(
        "name={} state={state}{code_part} pid={pid_str} restart={} runlevel={} \
         enabled={} disabled={} restarts={} exec={}",
        service.config.name,
        service.config.restart.as_str(),
        service.config.runlevel,
        service.config.enabled,
        service.disabled,
        service.restart_count,
        service.config.exec,
    )
}

pub fn format_all_services(services: &[ServiceState]) -> String {
    let mut output = String::new();
    for service in services {
        output.push_str(&format_service_status(service));
        output.push('\n');
    }
    output
}

pub fn find_service<'a>(services: &'a [ServiceState], name: &str) -> Option<&'a ServiceState> {
    services.iter().find(|service| service.config.name == name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        DEFAULT_RUNLEVEL, OutputMode, RestartPolicy, ServiceConfig, ServiceType, fallback_shell,
    };
    use crate::service::ServiceState;

    fn test_service(name: &str, status: RuntimeStatus, pid: Option<i32>) -> ServiceState {
        let config = ServiceConfig {
            name: name.to_string(),
            description: String::new(),
            exec: format!("/bin/{name}"),
            args: Vec::new(),
            enabled: true,
            runlevel: DEFAULT_RUNLEVEL.to_string(),
            restart: RestartPolicy::Always,
            service_type: ServiceType::Foreground,
            stdout: OutputMode::Console,
            stderr: OutputMode::Console,
        };
        let mut state = ServiceState::new(config);
        state.status = status;
        state.pid = pid;
        state
    }

    #[test]
    fn status_names_are_correct() {
        assert_eq!(runtime_status_name(RuntimeStatus::Stopped), "stopped");
        assert_eq!(runtime_status_name(RuntimeStatus::Starting), "starting");
        assert_eq!(runtime_status_name(RuntimeStatus::Running), "running");
        assert_eq!(runtime_status_name(RuntimeStatus::Exited(0)), "exited");
        assert_eq!(runtime_status_name(RuntimeStatus::Failed(1)), "failed");
    }

    #[test]
    fn formats_running_service() {
        let service = test_service("shell", RuntimeStatus::Running, Some(5));
        let line = format_service_status(&service);
        assert!(line.contains("name=shell"));
        assert!(line.contains("state=running"));
        assert!(line.contains("pid=5"));
        assert!(line.contains("restart=always"));
    }

    #[test]
    fn formats_failed_service_with_code() {
        let service = test_service("network", RuntimeStatus::Failed(127), None);
        let line = format_service_status(&service);
        assert!(line.contains("state=failed"));
        assert!(line.contains("code=127"));
        assert!(line.contains("pid=-1"));
    }

    #[test]
    fn ping_returns_ok_pong() {
        let services = [];
        let response = dispatch("PING", &services);
        assert_eq!(response.trim(), "OK pong");
    }

    #[test]
    fn unknown_command_returns_error() {
        let services = [];
        let response = dispatch("FROBNICATE", &services);
        assert!(response.starts_with("ERR"));
        assert!(response.contains("unsupported command"));
    }

    #[test]
    fn show_missing_returns_not_found() {
        let services = [test_service("shell", RuntimeStatus::Running, Some(5))];
        let response = dispatch("SHOW nonexistent", &services);
        assert!(response.starts_with("ERR"));
        assert!(response.contains("service not found"));
    }

    #[test]
    fn show_existing_returns_ok() {
        let services = [test_service("shell", RuntimeStatus::Running, Some(5))];
        let response = dispatch("SHOW shell", &services);
        assert!(response.starts_with("OK"));
        assert!(response.contains("name=shell"));
    }

    #[test]
    fn status_lists_all_services() {
        let services = [
            test_service("shell", RuntimeStatus::Running, Some(5)),
            test_service("network", RuntimeStatus::Running, Some(7)),
        ];
        let response = dispatch("STATUS", &services);
        assert!(response.starts_with("OK"));
        assert!(response.contains("name=shell"));
        assert!(response.contains("name=network"));
    }

    #[test]
    fn help_returns_command_list() {
        let response = dispatch("HELP", &[]);
        assert!(response.starts_with("OK"));
        assert!(response.contains("STATUS"));
        assert!(response.contains("PING"));
    }

    #[test]
    fn list_is_equivalent_to_status() {
        let services = [test_service("shell", RuntimeStatus::Running, Some(5))];
        let status = dispatch("STATUS", &services);
        let list = dispatch("LIST", &services);
        assert_eq!(status, list);
    }

    #[test]
    fn format_all_produces_one_line_per_service() {
        let services = [
            test_service("a", RuntimeStatus::Stopped, None),
            test_service("b", RuntimeStatus::Running, Some(10)),
        ];
        let output = format_all_services(&services);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn find_service_returns_match() {
        let shell = fallback_shell(DEFAULT_RUNLEVEL);
        let services = [ServiceState::new(shell)];
        assert!(find_service(&services, "fallback-shell").is_some());
        assert!(find_service(&services, "nonexistent").is_none());
    }
}
