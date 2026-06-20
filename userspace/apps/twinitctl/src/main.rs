use std::env;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

const TWINIT_CONTROL_SOCK: &str = "/run/twinit/control.sock";

#[derive(Debug, Clone, PartialEq, Eq)]
enum TwinitCommand {
    Status,
    List,
    Start(String),
    Stop(String),
    Restart(String),
    Reload(String),
    Enable(String),
    Disable(String),
    Show(String),
    Ping,
    Help,
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let encode_only = args.first().is_some_and(|arg| arg == "--encode");
    if encode_only {
        args.remove(0);
    }

    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("twinitctl: {error}");
            eprintln!("try 'twinitctl help' for usage");
            return ExitCode::from(2);
        }
    };

    if matches!(command, TwinitCommand::Help) && !encode_only {
        print_help();
        return ExitCode::SUCCESS;
    }

    let request = encode_command(&command);
    if encode_only {
        println!("{request}");
        return ExitCode::SUCCESS;
    }

    match send_command_to_twinit(&request) {
        Ok(response) => print_response(&response),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<TwinitCommand, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err("missing command".to_string());
    };

    match command {
        "help" | "--help" | "-h" => require_no_service(args, TwinitCommand::Help),
        "status" => require_no_service(args, TwinitCommand::Status),
        "list" => require_no_service(args, TwinitCommand::List),
        "ping" => require_no_service(args, TwinitCommand::Ping),
        "start" => service_command(args, "start", TwinitCommand::Start),
        "stop" => service_command(args, "stop", TwinitCommand::Stop),
        "restart" => service_command(args, "restart", TwinitCommand::Restart),
        "reload" => service_command(args, "reload", TwinitCommand::Reload),
        "enable" => service_command(args, "enable", TwinitCommand::Enable),
        "disable" => service_command(args, "disable", TwinitCommand::Disable),
        "show" => service_command(args, "show", TwinitCommand::Show),
        other => Err(format!("unknown command: {other}")),
    }
}

fn require_no_service(args: &[String], command: TwinitCommand) -> Result<TwinitCommand, String> {
    if args.len() == 1 {
        Ok(command)
    } else {
        Err(format!("{} does not accept a service name", args[0]))
    }
}

fn service_command(
    args: &[String],
    command_name: &str,
    constructor: fn(String) -> TwinitCommand,
) -> Result<TwinitCommand, String> {
    if args.len() != 2 || args[1].is_empty() {
        return Err(format!("{command_name} requires exactly one service name"));
    }
    if args[1].bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err("service name must not contain whitespace".to_string());
    }
    Ok(constructor(args[1].clone()))
}

fn encode_command(command: &TwinitCommand) -> String {
    match command {
        TwinitCommand::Status => "STATUS".to_string(),
        TwinitCommand::List => "LIST".to_string(),
        TwinitCommand::Start(name) => format!("START {name}"),
        TwinitCommand::Stop(name) => format!("STOP {name}"),
        TwinitCommand::Restart(name) => format!("RESTART {name}"),
        TwinitCommand::Reload(name) => format!("RELOAD {name}"),
        TwinitCommand::Enable(name) => format!("ENABLE {name}"),
        TwinitCommand::Disable(name) => format!("DISABLE {name}"),
        TwinitCommand::Show(name) => format!("SHOW {name}"),
        TwinitCommand::Ping => "PING".to_string(),
        TwinitCommand::Help => "HELP".to_string(),
    }
}

fn print_help() {
    print!(
        "{}",
        concat!(
            "usage: twinitctl <command> [service]\n",
            "\n",
            "commands:\n",
            "  status              show all services\n",
            "  list                list services\n",
            "  start NAME          start service\n",
            "  stop NAME           stop service\n",
            "  restart NAME        restart service\n",
            "  reload NAME         reload service\n",
            "  enable NAME         enable service\n",
            "  disable NAME        disable service\n",
            "  show NAME           show one service\n",
            "  ping                health check\n",
            "  help                show help\n",
        )
    );
}

fn send_command_to_twinit(request: &str) -> Result<String, String> {
    let mut stream = UnixStream::connect(TWINIT_CONTROL_SOCK)
        .map_err(|error| format!("twinitctl: cannot connect to {TWINIT_CONTROL_SOCK}: {error}"))?;

    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.write_all(b"\n"))
        .map_err(|error| format!("twinitctl: failed to send request: {error}"))?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|error| format!("twinitctl: failed to finish request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("twinitctl: failed to read response: {error}"))?;
    if response.trim().is_empty() {
        return Err("twinitctl: twinit returned an empty response".to_string());
    }
    Ok(response)
}

fn print_response(response: &str) -> ExitCode {
    let response = response.trim_end();
    if response == "ERR" || response.starts_with("ERR ") || response.starts_with("ERR\n") {
        eprintln!("{response}");
        return ExitCode::FAILURE;
    }

    if let Some(body) = response.strip_prefix("OK\n") {
        if !body.is_empty() {
            println!("{body}");
        }
    } else {
        println!("{response}");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse_args(&strings(&["status"])), Ok(TwinitCommand::Status));
        assert_eq!(parse_args(&strings(&["list"])), Ok(TwinitCommand::List));
        assert_eq!(
            parse_args(&strings(&["start", "network"])),
            Ok(TwinitCommand::Start("network".to_string()))
        );
        assert_eq!(
            parse_args(&strings(&["show", "shell"])),
            Ok(TwinitCommand::Show("shell".to_string()))
        );
        assert_eq!(parse_args(&strings(&["--help"])), Ok(TwinitCommand::Help));
        assert_eq!(parse_args(&strings(&["ping"])), Ok(TwinitCommand::Ping));
    }

    #[test]
    fn rejects_bad_arity() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&strings(&["start"])).is_err());
        assert!(parse_args(&strings(&["status", "shell"])).is_err());
        assert!(parse_args(&strings(&["stop", "two words"])).is_err());
        assert!(parse_args(&strings(&["ping", "extra"])).is_err());
    }

    #[test]
    fn encodes_protocol_lines() {
        assert_eq!(
            encode_command(&TwinitCommand::Restart("network".to_string())),
            "RESTART network"
        );
        assert_eq!(encode_command(&TwinitCommand::Status), "STATUS");
        assert_eq!(encode_command(&TwinitCommand::Ping), "PING");
    }
}
