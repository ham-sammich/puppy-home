//! The "Add MCP server" wizard (modal), split out of `mcp_manager.rs`.
//!
//! Two ways in: a guided **Form** (transport, fields, review) or a raw
//! **Paste** mode where you drop in a server entry like
//! `{"my-server": {"command": "npx", "args": [...]}}` (an outer
//! `mcpServers`/`mcp_servers` wrapper is unwrapped, and the transport is
//! inferred from `command` vs `url`). Both produce the same name + transport +
//! config the manager hands to `add_mcp_server`. Per-step renderers live in the
//! `steps` child module.

use serde_json::{Map, Value, json};

use crate::views::common::{EditMode, validate_name};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Transport {
    Stdio,
    Sse,
    Http,
}

impl Transport {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Transport::Stdio => "stdio",
            Transport::Sse => "sse",
            Transport::Http => "http",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Transport::Stdio => "Command (stdio)",
            Transport::Sse => "Remote URL (SSE)",
            Transport::Http => "Remote URL (HTTP)",
        }
    }

    pub(crate) fn blurb(self) -> &'static str {
        match self {
            Transport::Stdio => "Run a local process; talk over stdin/stdout.",
            Transport::Sse => "Connect to a server-sent-events endpoint.",
            Transport::Http => "Connect to a streamable-HTTP endpoint.",
        }
    }
}

/// The guided "Add MCP server" wizard state.
pub struct Wizard {
    // Fields are pub(crate) so the GPUI manager drives the same state
    // machine (sync note: mirror on the egui branch at batch time).
    pub(crate) step: usize,
    pub(crate) transport: Transport,
    pub(crate) name: String,
    /// One argument per line.
    pub(crate) command: String,
    pub(crate) args: String,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) error: Option<String>,
    /// Form (guided steps) vs. Paste (drop in a server entry and validate).
    pub(crate) mode: EditMode,
    /// The raw-paste buffer, seeded from the form on entry.
    pub(crate) paste: String,
}

impl Wizard {
    pub fn new() -> Self {
        Wizard {
            step: 0,
            transport: Transport::Stdio,
            name: String::new(),
            command: String::new(),
            args: String::new(),
            env: Vec::new(),
            url: String::new(),
            headers: Vec::new(),
            error: None,
            mode: EditMode::Form,
            paste: String::new(),
        }
    }

    /// The validated server name (trimmed) — for `add_mcp_server`.
    pub fn name(&self) -> String {
        self.name.trim().to_string()
    }

    /// The wire transport string (`stdio`/`sse`/`http`).
    pub fn transport_wire(&self) -> &'static str {
        self.transport.wire()
    }

    /// The server config object (`command`/`args`/`env` or `url`/`headers`).
    pub fn config(&self) -> Value {
        build_config(self)
    }

    /// Seed the paste buffer from the current form (a `{name: config}` entry).
    pub(crate) fn sync_paste_from_form(&mut self) {
        if self.name.trim().is_empty() {
            self.paste = "{\n  \"my-server\": {\n    \"type\": \"stdio\",\n    \
                          \"command\": \"npx\",\n    \"args\": [\"-y\", \"some-mcp-server\"]\n  }\n}"
                .to_string();
            return;
        }
        let mut cfg = build_config(self);
        if let Value::Object(map) = &mut cfg {
            map.insert("type".into(), json!(self.transport.wire()));
        }
        let mut wrapper = Map::new();
        wrapper.insert(self.name.trim().to_string(), cfg);
        self.paste = serde_json::to_string_pretty(&Value::Object(wrapper)).unwrap_or_default();
    }

    /// Parse the paste buffer back into the form fields (the syntax check).
    pub(crate) fn apply_paste(&mut self) -> Result<(), String> {
        let p = parse_paste(&self.paste)?;
        validate_name(&p.name)?;
        self.name = p.name;
        self.transport = p.transport;
        self.command = p.command;
        self.args = p.args;
        self.env = p.env;
        self.url = p.url;
        self.headers = p.headers;
        validate_fields(self)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Split the args textarea: one argument per line, trimmed, empties dropped.
fn split_args(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Key/value rows -> JSON object, skipping rows with an empty key.
fn pairs_to_map(rows: &[(String, String)]) -> Map<String, Value> {
    rows.iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), Value::String(v.clone())))
        .collect()
}

/// Build the Code Puppy server config object from the wizard's fields.
fn build_config(w: &Wizard) -> Value {
    match w.transport {
        Transport::Stdio => {
            let mut obj = Map::new();
            obj.insert("command".into(), json!(w.command.trim()));
            let args = split_args(&w.args);
            if !args.is_empty() {
                obj.insert("args".into(), json!(args));
            }
            let env = pairs_to_map(&w.env);
            if !env.is_empty() {
                obj.insert("env".into(), Value::Object(env));
            }
            Value::Object(obj)
        }
        Transport::Sse | Transport::Http => {
            let mut obj = Map::new();
            obj.insert("url".into(), json!(w.url.trim()));
            let headers = pairs_to_map(&w.headers);
            if !headers.is_empty() {
                obj.insert("headers".into(), Value::Object(headers));
            }
            Value::Object(obj)
        }
    }
}

/// Validate the fields; mirrors Code Puppy's registry validation so the user
/// hears about problems before the op crosses the wire.
pub(crate) fn validate_fields(w: &Wizard) -> Result<(), String> {
    validate_name(&w.name)?;
    match w.transport {
        Transport::Stdio => {
            if w.command.trim().is_empty() {
                return Err("a command is required for a stdio server".into());
            }
        }
        Transport::Sse | Transport::Http => {
            let url = w.url.trim();
            if url.is_empty() {
                return Err("a URL is required".into());
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("URL must start with http:// or https://".into());
            }
        }
    }
    Ok(())
}

/// One server parsed out of a pasted `{name: config}` entry.
struct ParsedServer {
    name: String,
    transport: Transport,
    command: String,
    args: String,
    env: Vec<(String, String)>,
    url: String,
    headers: Vec<(String, String)>,
}

/// Object -> editable key/value rows (insertion order; string values).
fn map_to_pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let val = v
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string());
                    (k.clone(), val)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a pasted server entry into a [`ParsedServer`]. Accepts an outer
/// `mcpServers`/`mcp_servers`/`servers` wrapper and a single `{name: config}`
/// entry; infers transport from an explicit `type`/`transport` or from
/// `command` vs `url`.
fn parse_paste(text: &str) -> Result<ParsedServer, String> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| format!("invalid JSON: {e}"))?;
    let mut obj = v
        .as_object()
        .ok_or("expected a JSON object like {\"name\": { ... }}")?
        .clone();

    // Unwrap a common outer wrapper (Claude-desktop style configs).
    for key in ["mcpServers", "mcp_servers", "servers"] {
        if obj.len() == 1
            && let Some(inner) = obj.get(key).and_then(Value::as_object)
        {
            obj = inner.clone();
            break;
        }
    }

    if obj.len() != 1 {
        return Err("paste a single server as {\"name\": { ...config... }}".into());
    }
    let (name, cfg) = obj.iter().next().unwrap();
    let cfg = cfg
        .as_object()
        .ok_or("the server config must be a JSON object")?;

    let explicit = cfg
        .get("type")
        .or_else(|| cfg.get("transport"))
        .and_then(Value::as_str);
    let transport = match explicit {
        Some("stdio") => Transport::Stdio,
        Some("sse") => Transport::Sse,
        Some("http") | Some("streamable-http") | Some("streamable_http") => Transport::Http,
        _ if cfg.contains_key("command") => Transport::Stdio,
        _ if cfg.contains_key("url") => Transport::Http,
        _ => {
            return Err("config needs a \"command\" (stdio) or \"url\" (sse/http)".into());
        }
    };

    let str_field = |k: &str| cfg.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let args = cfg
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    Ok(ParsedServer {
        name: name.clone(),
        transport,
        command: str_field("command"),
        args,
        env: map_to_pairs(cfg.get("env")),
        url: str_field("url"),
        headers: map_to_pairs(cfg.get("headers")),
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio_wizard() -> Wizard {
        let mut w = Wizard::new();
        w.name = "fs".into();
        w.command = " npx ".into();
        w.args = "-y\n\n  server-filesystem  \n".into();
        w.env = vec![
            ("KEY".into(), "value".into()),
            ("".into(), "ignored".into()),
        ];
        w
    }

    #[test]
    fn args_split_one_per_line_trimmed() {
        assert_eq!(split_args("-y\n\n  server  \n"), vec!["-y", "server"]);
        assert!(split_args("").is_empty());
    }

    #[test]
    fn stdio_config_shape() {
        let cfg = build_config(&stdio_wizard());
        assert_eq!(
            cfg,
            json!({
                "command": "npx",
                "args": ["-y", "server-filesystem"],
                "env": {"KEY": "value"}
            })
        );
    }

    #[test]
    fn url_config_shape() {
        let mut w = Wizard::new();
        w.transport = Transport::Sse;
        w.name = "remote".into();
        w.url = " https://example.com/sse ".into();
        w.headers = vec![("Authorization".into(), "Bearer x".into())];
        assert_eq!(
            build_config(&w),
            json!({"url": "https://example.com/sse", "headers": {"Authorization": "Bearer x"}})
        );
    }

    #[test]
    fn field_validation_per_transport() {
        let mut w = Wizard::new();
        w.name = "ok".into();
        assert!(validate_fields(&w).is_err()); // stdio without command
        w.command = "npx".into();
        assert!(validate_fields(&w).is_ok());
        w.transport = Transport::Http;
        assert!(validate_fields(&w).is_err()); // http without url
        w.url = "ftp://nope".into();
        assert!(validate_fields(&w).is_err()); // bad scheme
        w.url = "http://localhost:9000".into();
        assert!(validate_fields(&w).is_ok());
    }

    #[test]
    fn paste_round_trips_stdio() {
        let mut w = stdio_wizard();
        w.sync_paste_from_form();
        let mut blank = Wizard::new();
        blank.paste = w.paste.clone();
        blank.apply_paste().unwrap();
        assert_eq!(blank.name, "fs");
        assert_eq!(blank.transport, Transport::Stdio);
        assert_eq!(blank.command.trim(), "npx");
        assert_eq!(split_args(&blank.args), vec!["-y", "server-filesystem"]);
    }

    #[test]
    fn paste_unwraps_mcp_servers_wrapper_and_infers_stdio() {
        let mut w = Wizard::new();
        w.paste =
            "{\"mcpServers\": {\"fs\": {\"command\": \"uvx\", \"args\": [\"a\", \"b\"]}}}".into();
        w.apply_paste().unwrap();
        assert_eq!(w.name, "fs");
        assert_eq!(w.transport, Transport::Stdio);
        assert_eq!(split_args(&w.args), vec!["a", "b"]);
    }

    #[test]
    fn paste_infers_transport_and_reads_type() {
        let mut w = Wizard::new();
        w.paste = "{\"remote\": {\"type\": \"sse\", \"url\": \"https://x/y\"}}".into();
        w.apply_paste().unwrap();
        assert_eq!(w.transport, Transport::Sse);
        assert_eq!(w.url, "https://x/y");
    }

    #[test]
    fn paste_rejects_bad_input() {
        let mut w = Wizard::new();
        w.paste = "{ not json".into();
        assert!(w.apply_paste().is_err());
        w.paste = "{\"a\": {}, \"b\": {}}".into();
        assert!(w.apply_paste().is_err()); // more than one server
        w.paste = "{\"ok\": {\"foo\": 1}}".into();
        assert!(w.apply_paste().is_err()); // neither command nor url
        w.paste = "{\"bad/name\": {\"command\": \"x\"}}".into();
        assert!(w.apply_paste().is_err()); // bad name
    }
}
