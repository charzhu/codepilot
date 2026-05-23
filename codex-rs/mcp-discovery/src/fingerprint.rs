//! Compute a stable, content-based fingerprint for an MCP server config.
//!
//! The fingerprint intentionally ignores env vars, bearer tokens, and HTTP
//! headers so two entries with the same effective command target collide even
//! when one author sets `GITHUB_TOKEN` explicitly and another reads it from
//! the environment.

use std::path::Path;

use codex_config::McpServerConfig;
use codex_config::McpServerTransportConfig;
use sha2::Digest;
use sha2::Sha256;

use crate::normalize::normalize_cwd;
use crate::normalize::normalize_exe;
use crate::normalize::normalize_url;

/// Compute the discovery fingerprint for `config`. Two configs are considered
/// duplicates when their fingerprints match exactly. See the module-level
/// rationale for what gets included.
pub fn fingerprint(config: &McpServerConfig) -> String {
    let mut hasher = Sha256::new();
    match &config.transport {
        McpServerTransportConfig::Stdio {
            command, args, cwd, ..
        } => {
            hasher.update(b"stdio\0");
            hasher.update(normalize_exe(command).as_bytes());
            hasher.update(b"\0");
            for arg in args {
                hasher.update(arg.as_bytes());
                hasher.update(b"\0");
            }
            hasher.update(b"\0");
            let cwd_normalized = cwd
                .as_ref()
                .map(|path| normalize_cwd(path))
                .unwrap_or_default();
            hasher.update(cwd_normalized.as_bytes());
        }
        McpServerTransportConfig::StreamableHttp { url, .. } => {
            hasher.update(b"http\0");
            hasher.update(normalize_url(url).as_bytes());
        }
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Returns true if the resolved command (or URL host) appears to point at
/// Codex itself. Used to drop self-referential entries that would create a
/// proxy loop on startup.
pub fn is_self_reference(config: &McpServerConfig, additional_self_names: &[&str]) -> bool {
    match &config.transport {
        McpServerTransportConfig::Stdio { command, args, .. } => {
            let normalized = normalize_exe(command);
            if matches!(
                normalized.as_str(),
                "codex" | "codex-mcp" | "codex-mcp-server"
            ) {
                return true;
            }
            for arg in args {
                let path = Path::new(arg);
                if let Some(file) = path.file_name().and_then(|s| s.to_str())
                    && additional_self_names
                        .iter()
                        .any(|needle| file.eq_ignore_ascii_case(needle))
                {
                    return true;
                }
            }
            false
        }
        McpServerTransportConfig::StreamableHttp { url, .. } => {
            let url_lower = url.to_ascii_lowercase();
            url_lower.contains("127.0.0.1") || url_lower.contains("localhost")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use codex_config::McpServerConfig;
    use codex_config::McpServerTransportConfig;
    use pretty_assertions::assert_eq;

    use super::fingerprint;
    use super::is_self_reference;

    fn stdio(command: &str, args: Vec<&str>) -> McpServerConfig {
        McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: command.to_string(),
                args: args.into_iter().map(String::from).collect(),
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
            enabled: true,
            required: false,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            enabled_tools: None,
            disabled_tools: None,
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        }
    }

    fn http(url: &str) -> McpServerConfig {
        McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: url.to_string(),
                bearer_token_env_var: None,
                http_headers: None,
                env_http_headers: None,
            },
            environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
            enabled: true,
            required: false,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            enabled_tools: None,
            disabled_tools: None,
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        }
    }

    #[test]
    fn stdio_paths_with_same_basename_match() {
        let a = stdio("python", vec!["-m", "server"]);
        let b = stdio("/usr/bin/python", vec!["-m", "server"]);
        let c = stdio("C:\\Python\\python.exe", vec!["-m", "server"]);
        assert_eq!(fingerprint(&a), fingerprint(&b));
        assert_eq!(fingerprint(&a), fingerprint(&c));
    }

    #[test]
    fn different_args_break_match() {
        let a = stdio("python", vec!["-m", "server"]);
        let b = stdio("python", vec!["-m", "other"]);
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn http_url_dedup_ignores_default_port_and_case() {
        let a = http("HTTPS://Example.com:443/foo/");
        let b = http("https://example.com/foo");
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn self_reference_detects_codex_binaries() {
        let config = stdio("codex", vec!["mcp"]);
        assert!(is_self_reference(&config, &[]));
        let config = stdio("/opt/codex/codex-mcp.exe", vec![]);
        assert!(is_self_reference(&config, &[]));
        // Bare arg matches because the file-name component IS "openjaw-agent".
        let config = stdio("python", vec!["openjaw-agent"]);
        assert!(is_self_reference(&config, &["openjaw-agent"]));
        // Path argument with matching basename matches as well.
        let config = stdio("python", vec!["foo/openjaw-agent"]);
        assert!(is_self_reference(&config, &["openjaw-agent"]));
        // Args that do not contain the needle as a basename are left alone.
        let config = stdio("python", vec!["server.py"]);
        assert!(!is_self_reference(&config, &["openjaw-agent"]));
    }

    #[test]
    fn self_reference_detects_localhost_urls() {
        let config = http("http://localhost:8080/mcp");
        assert!(is_self_reference(&config, &[]));
        let config = http("https://api.example.com/mcp");
        assert!(!is_self_reference(&config, &[]));
    }
}
