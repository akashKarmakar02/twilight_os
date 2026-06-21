//! Service configuration parsing for twinit.
//!
//! Reads `.toml` service files from `/etc/twinit/services/`, validates
//! fields, and produces `ServiceConfig` values ready for supervision.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub const SERVICE_DIR: &str = "/etc/twinit/services";
pub const DEFAULT_RUNLEVEL: &str = "default";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
}

impl RestartPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnFailure => "on-failure",
            Self::Always => "always",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ServiceType {
    Foreground,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Console,
    Null,
    Log,
}

#[derive(Debug)]
pub struct ServiceConfig {
    pub name: String,
    pub description: String,
    pub exec: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub runlevel: String,
    pub restart: RestartPolicy,
    pub service_type: ServiceType,
    pub stdout: OutputMode,
    pub stderr: OutputMode,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

pub fn load_service_configs(directory: &Path, runlevel: &str) -> Vec<ServiceConfig> {
    let Ok(entries) = fs::read_dir(directory) else {
        println!(
            "twinit: service directory {} not found",
            directory.display()
        );
        return Vec::new();
    };

    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    paths.sort();

    let mut configs = Vec::new();
    for path in paths {
        match fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|contents| parse_service_config(&contents))
        {
            Ok(config) if config.enabled && config.runlevel == runlevel => {
                println!(
                    "twinit: loaded service {} ({})",
                    config.name, config.description
                );
                configs.push(config);
            }
            Ok(_) => {}
            Err(error) => eprintln!("twinit: ignoring {}: {error}", path.display()),
        }
    }
    configs
}

pub fn fallback_shell(runlevel: &str) -> ServiceConfig {
    ServiceConfig {
        name: "fallback-shell".to_string(),
        description: "Built-in interactive fallback shell".to_string(),
        exec: "/bin/tsh".to_string(),
        args: Vec::new(),
        enabled: true,
        runlevel: runlevel.to_string(),
        restart: RestartPolicy::Always,
        service_type: ServiceType::Foreground,
        stdout: OutputMode::Console,
        stderr: OutputMode::Console,
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

pub fn parse_service_config(contents: &str) -> Result<ServiceConfig, String> {
    let mut fields = HashMap::<String, String>::new();
    for (index, original_line) in contents.lines().enumerate() {
        let line = strip_comment(original_line).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected key = value", index + 1));
        };
        let key = key.trim();
        if !matches!(
            key,
            "name"
                | "description"
                | "exec"
                | "args"
                | "enabled"
                | "runlevel"
                | "restart"
                | "type"
                | "stdout"
                | "stderr"
        ) {
            return Err(format!("line {}: unsupported field {key}", index + 1));
        }
        if fields
            .insert(key.to_string(), value.trim().to_string())
            .is_some()
        {
            return Err(format!("line {}: duplicate field {key}", index + 1));
        }
    }

    let name = required_string(&fields, "name")?;
    let exec = required_string(&fields, "exec")?;
    if !exec.starts_with('/') {
        return Err("exec must be an absolute path".to_string());
    }
    let description = optional_string(&fields, "description")?.unwrap_or_default();
    let args = match fields.get("args") {
        Some(value) => parse_string_array(value)?,
        None => Vec::new(),
    };
    let enabled = match fields.get("enabled") {
        Some(value) => parse_bool(value)?,
        None => true,
    };
    let runlevel =
        optional_string(&fields, "runlevel")?.unwrap_or_else(|| DEFAULT_RUNLEVEL.to_string());
    if !matches!(
        runlevel.as_str(),
        "boot" | "default" | "single" | "shutdown"
    ) {
        return Err(format!("unsupported runlevel {runlevel}"));
    }

    let restart = match optional_string(&fields, "restart")?
        .as_deref()
        .unwrap_or("never")
    {
        "never" => RestartPolicy::Never,
        "on-failure" => RestartPolicy::OnFailure,
        "always" => RestartPolicy::Always,
        value => return Err(format!("unsupported restart policy {value}")),
    };
    let service_type = match optional_string(&fields, "type")?
        .as_deref()
        .unwrap_or("foreground")
    {
        "foreground" => ServiceType::Foreground,
        value => return Err(format!("unsupported service type {value}")),
    };
    let stdout = parse_output_mode(optional_string(&fields, "stdout")?.as_deref())?;
    let stderr = parse_output_mode(optional_string(&fields, "stderr")?.as_deref())?;

    Ok(ServiceConfig {
        name,
        description,
        exec,
        args,
        enabled,
        runlevel,
        restart,
        service_type,
        stdout,
        stderr,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn required_string(fields: &HashMap<String, String>, name: &str) -> Result<String, String> {
    optional_string(fields, name)?.ok_or_else(|| format!("missing required field {name}"))
}

fn optional_string(fields: &HashMap<String, String>, name: &str) -> Result<Option<String>, String> {
    fields
        .get(name)
        .map(|value| parse_string(value).map(Some))
        .unwrap_or(Ok(None))
}

fn parse_string(value: &str) -> Result<String, String> {
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err(format!("expected quoted string, got {value}"));
    }
    let mut result = String::new();
    let mut escaped = false;
    for character in value[1..value.len() - 1].chars() {
        if escaped {
            result.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                other => return Err(format!("unsupported escape \\{other}")),
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            result.push(character);
        }
    }
    if escaped {
        return Err("unterminated string escape".to_string());
    }
    Ok(result)
}

fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
    let value = value.trim();
    if !value.starts_with('[') || !value.ends_with(']') {
        return Err(format!("expected string array, got {value}"));
    }
    let body = value[1..value.len() - 1].trim();
    if body.is_empty() {
        return Ok(Vec::new());
    }

    let mut values = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in body.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quoted {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == ',' && !quoted {
            values.push(parse_string(body[start..index].trim())?);
            start = index + 1;
        }
    }
    if quoted {
        return Err("unterminated quoted string in args".to_string());
    }
    values.push(parse_string(body[start..].trim())?);
    Ok(values)
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("expected boolean, got {value}")),
    }
}

fn parse_output_mode(value: Option<&str>) -> Result<OutputMode, String> {
    match value.unwrap_or("console") {
        "console" => Ok(OutputMode::Console),
        "null" => Ok(OutputMode::Null),
        "log" => Ok(OutputMode::Log),
        other => Err(format!("unsupported output mode {other}")),
    }
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quoted {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == '#' && !quoted {
            return &line[..index];
        }
    }
    line
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_service_file() {
        let config = parse_service_config(
            r#"
                name = "demo"
                description = "Demo #1"
                exec = "/bin/demo"
                args = ["one", "two words"]
                enabled = true
                runlevel = "default"
                restart = "on-failure"
                type = "foreground"
                stdout = "null"
                stderr = "log"
            "#,
        )
        .unwrap();
        assert_eq!(config.name, "demo");
        assert_eq!(config.args, ["one", "two words"]);
        assert!(matches!(config.restart, RestartPolicy::OnFailure));
        assert_eq!(config.stdout, OutputMode::Null);
        assert_eq!(config.stderr, OutputMode::Log);
    }

    #[test]
    fn rejects_missing_name() {
        let result = parse_service_config(r#"exec = "/bin/demo""#);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_relative_exec() {
        let result = parse_service_config(
            r#"
                name = "bad"
                exec = "relative/path"
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_duplicate_field() {
        let result = parse_service_config(
            r#"
                name = "dup"
                name = "dup2"
                exec = "/bin/x"
            "#,
        );
        assert!(result.is_err());
    }
}
