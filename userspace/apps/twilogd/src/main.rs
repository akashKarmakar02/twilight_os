use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::net::UnixDatagram;
use std::path::Path;

const TWILOG_RUNTIME_DIR: &str = "/run/twilight";
const TWILOG_SOCKET: &str = "/run/twilight/log.sock";
const TWILOG_FILE: &str = "/var/log/twilight.log";
const TWILOG_FALLBACK_FILE: &str = "/run/twilight/twilight.log";
const MAX_DATAGRAM: usize = 4096;

fn main() {
    if let Err(error) = run() {
        eprintln!("twilogd: fatal: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    println!("twilogd: starting");
    ensure_directory("/run")?;
    ensure_directory(TWILOG_RUNTIME_DIR)?;

    // `/var` may be read-only or absent on early boot. Failure to create its
    // log directory is intentionally non-fatal because the runtime file is
    // always available as a fallback.
    let _ = ensure_directory("/var");
    let _ = ensure_directory("/var/log");
    let (mut log_file, log_path) = open_log_file()?;

    match fs::remove_file(TWILOG_SOCKET) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let socket = UnixDatagram::bind(TWILOG_SOCKET)?;

    println!("twilogd: listening on {TWILOG_SOCKET}");
    println!("twilogd: writing to {log_path}");

    let mut counter = 0_u64;
    let mut buffer = [0_u8; MAX_DATAGRAM];
    loop {
        let received = match socket.recv(&mut buffer) {
            Ok(received) => received,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };

        counter = counter.saturating_add(1);
        let datagram = String::from_utf8_lossy(&buffer[..received]);
        let entry = parse_entry(&datagram);
        writeln!(
            log_file,
            "[{counter:06}] level={} source={} message={}",
            sanitize_field(&entry.level),
            sanitize_field(&entry.source),
            sanitize_message(&entry.message)
        )?;
        log_file.flush()?;
    }
}

fn ensure_directory(path: &str) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if Path::new(path).is_dir() {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

fn open_log_file() -> io::Result<(File, &'static str)> {
    match open_append(TWILOG_FILE) {
        Ok(file) => Ok((file, TWILOG_FILE)),
        Err(primary_error) => open_append(TWILOG_FALLBACK_FILE)
            .map(|file| (file, TWILOG_FALLBACK_FILE))
            .map_err(|fallback_error| {
                io::Error::new(
                    fallback_error.kind(),
                    format!(
                        "cannot open {TWILOG_FILE} ({primary_error}) or \
                         {TWILOG_FALLBACK_FILE} ({fallback_error})"
                    ),
                )
            }),
    }
}

fn open_append(path: &str) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

#[derive(Debug, PartialEq, Eq)]
struct LogEntry {
    level: String,
    source: String,
    message: String,
}

fn parse_entry(datagram: &str) -> LogEntry {
    let raw = datagram.trim_matches(['\0', '\r', '\n']);
    let mut level = "INFO";
    let mut source = "unknown";

    let message = if let Some(message_index) = field_index(raw, "MESSAGE=") {
        let fields = raw[..message_index].trim_end();
        for field in fields.split_whitespace() {
            if let Some(value) = field.strip_prefix("LEVEL=") {
                if !value.is_empty() {
                    level = value;
                }
            } else if let Some(value) = field.strip_prefix("SOURCE=") {
                if !value.is_empty() {
                    source = value;
                }
            }
        }
        &raw[message_index + "MESSAGE=".len()..]
    } else {
        raw
    };

    LogEntry {
        level: level.to_string(),
        source: source.to_string(),
        message: message.to_string(),
    }
}

fn field_index(input: &str, field: &str) -> Option<usize> {
    if input.starts_with(field) {
        return Some(0);
    }
    input.find(&format!(" {field}")).map(|index| index + 1)
}

fn sanitize_field(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_whitespace() || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect()
}

fn sanitize_message(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\r' | '\n' => ' ',
            character if character.is_control() => '?',
            character => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_entry() {
        assert_eq!(
            parse_entry("LEVEL=ERROR SOURCE=httpd MESSAGE=bind failed"),
            LogEntry {
                level: "ERROR".to_string(),
                source: "httpd".to_string(),
                message: "bind failed".to_string(),
            }
        );
    }

    #[test]
    fn parses_raw_entry_with_defaults() {
        assert_eq!(
            parse_entry("hello from service\n"),
            LogEntry {
                level: "INFO".to_string(),
                source: "unknown".to_string(),
                message: "hello from service".to_string(),
            }
        );
    }
}
