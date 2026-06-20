use std::env;
use std::fs;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::ExitCode;

const TWILOG_SOCKET: &str = "/run/twilight/log.sock";
const TWILOG_FILE: &str = "/var/log/twilight.log";
const TWILOG_FALLBACK_FILE: &str = "/run/twilight/twilight.log";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("twilogctl: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return Err("missing command".to_string());
    };

    match command {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "send" => {
            if args.len() < 2 {
                return Err("send requires a message".to_string());
            }
            send_message(&args[1..].join(" "))
        }
        "status" if args.len() == 1 => show_status(),
        "show" | "tail" if args.len() == 1 => show_log(),
        "status" | "show" | "tail" => Err(format!("{command} takes no arguments")),
        other => Err(format!("unknown command: {other}")),
    }
}

fn send_message(message: &str) -> Result<(), String> {
    let message = message.replace(['\r', '\n'], " ");
    let datagram = format!("LEVEL=INFO SOURCE=twilogctl MESSAGE={message}");
    let socket = UnixDatagram::unbound().map_err(|error| error.to_string())?;
    socket
        .send_to(datagram.as_bytes(), TWILOG_SOCKET)
        .map_err(|error| format!("cannot send to {TWILOG_SOCKET}: {error}"))?;
    println!("twilogctl: sent");
    Ok(())
}

fn show_status() -> Result<(), String> {
    if !Path::new(TWILOG_SOCKET).exists() {
        return Err(format!("logger unavailable: {TWILOG_SOCKET}"));
    }
    let log_path = current_log_path().unwrap_or("not-created");
    println!("twilogd: socket={TWILOG_SOCKET} log={log_path}");
    Ok(())
}

fn show_log() -> Result<(), String> {
    let (contents, path) = read_current_log().map_err(|error| error.to_string())?;
    print!("{contents}");
    if contents.is_empty() {
        eprintln!("twilogctl: log is empty: {path}");
    }
    Ok(())
}

fn current_log_path() -> Option<&'static str> {
    [TWILOG_FILE, TWILOG_FALLBACK_FILE]
        .into_iter()
        .find(|path| Path::new(path).is_file())
}

fn read_current_log() -> io::Result<(String, &'static str)> {
    let mut last_error = None;
    for path in [TWILOG_FILE, TWILOG_FALLBACK_FILE] {
        match fs::read_to_string(path) {
            Ok(contents) => return Ok((contents, path)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::from(io::ErrorKind::NotFound)))
}

fn print_help() {
    println!(
        "usage: twilogctl <command> [message]\n\n\
         commands:\n  \
           send MESSAGE...    send a log entry\n  \
           status             show logger paths\n  \
           show               show the current log\n  \
           tail               show the current log\n  \
           help               show help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_is_accepted() {
        assert!(run(vec!["help".to_string()]).is_ok());
    }

    #[test]
    fn send_requires_message() {
        assert!(run(vec!["send".to_string()]).is_err());
    }
}
