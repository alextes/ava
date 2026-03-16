use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::Error;

#[derive(Debug, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// load MCP config from ~/.ava/mcp.toml (or AVA_HOME/mcp.toml).
/// returns an empty config if the file doesn't exist.
pub fn load_mcp_config() -> Result<McpConfig, Error> {
    let path = mcp_config_path();
    if !path.exists() {
        tracing::debug!(path = %path.display(), "no MCP config file found");
        return Ok(McpConfig {
            mcp_servers: vec![],
        });
    }

    let contents = std::fs::read_to_string(&path)?;
    let config: McpConfig =
        toml::from_str(&contents).map_err(|e| Error::Mcp(format!("invalid mcp config: {e}")))?;

    let enabled_count = config.mcp_servers.iter().filter(|s| s.enabled).count();
    tracing::info!(
        path = %path.display(),
        total = config.mcp_servers.len(),
        enabled = enabled_count,
        "loaded MCP config"
    );

    Ok(config)
}

fn mcp_config_path() -> PathBuf {
    crate::config::ava_home_dir().join("mcp.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mcp_config() {
        let toml = r#"
[[mcp_servers]]
name = "github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_abc123" }

[[mcp_servers]]
name = "sqlite"
command = "uvx"
args = ["mcp-server-sqlite", "--db-path", "./data.db"]

[[mcp_servers]]
name = "disabled-server"
command = "some-cmd"
enabled = false
"#;

        let config: McpConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.mcp_servers.len(), 3);

        let github = &config.mcp_servers[0];
        assert_eq!(github.name, "github");
        assert_eq!(github.command, "npx");
        assert_eq!(
            github.args,
            vec!["-y", "@modelcontextprotocol/server-github"]
        );
        assert_eq!(github.env.get("GITHUB_TOKEN").unwrap(), "ghp_abc123");
        assert!(github.enabled);

        let sqlite = &config.mcp_servers[1];
        assert_eq!(sqlite.name, "sqlite");
        assert!(sqlite.enabled);
        assert!(sqlite.env.is_empty());

        let disabled = &config.mcp_servers[2];
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_parse_empty_config() {
        let config: McpConfig = toml::from_str("").unwrap();
        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn test_parse_minimal_server() {
        let toml = r#"
[[mcp_servers]]
name = "minimal"
command = "my-server"
"#;
        let config: McpConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.mcp_servers.len(), 1);
        let server = &config.mcp_servers[0];
        assert_eq!(server.name, "minimal");
        assert!(server.args.is_empty());
        assert!(server.env.is_empty());
        assert!(server.enabled);
    }
}
