use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct LauncherConfig {
    pub backend_url: Option<String>,
    pub auth_token: Option<String>,
    pub name: Option<String>,
    pub max_processes: Option<usize>,
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("claude-portal")
        .join("launcher.toml")
}

pub fn load_config() -> LauncherConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(config) => {
                tracing::info!("Loaded config from {}", path.display());
                config
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {}", path.display(), e);
                LauncherConfig::default()
            }
        },
        Err(_) => LauncherConfig::default(),
    }
}

pub fn save_auth_token(token: &str) -> anyhow::Result<()> {
    let path = config_path();
    let mut config: LauncherConfig = match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
        Err(_) => LauncherConfig::default(),
    };

    config.auth_token = Some(token.to_string());

    let contents = toml::to_string_pretty(&config)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, contents)?;
    tracing::info!("Saved auth token to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let toml = r#"
backend_url = "wss://example.com"
auth_token = "tok_abc123"
name = "my-launcher"
max_processes = 10
"#;
        let config: LauncherConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.backend_url.unwrap(), "wss://example.com");
        assert_eq!(config.auth_token.unwrap(), "tok_abc123");
        assert_eq!(config.name.unwrap(), "my-launcher");
        assert_eq!(config.max_processes.unwrap(), 10);
    }

    #[test]
    fn parse_empty_config() {
        let config: LauncherConfig = toml::from_str("").unwrap();
        assert!(config.backend_url.is_none());
        assert!(config.auth_token.is_none());
        assert!(config.name.is_none());
        assert!(config.max_processes.is_none());
    }

    #[test]
    fn parse_partial_config() {
        let toml = r#"
auth_token = "secret"
"#;
        let config: LauncherConfig = toml::from_str(toml).unwrap();
        assert!(config.backend_url.is_none());
        assert_eq!(config.auth_token.unwrap(), "secret");
    }

    #[test]
    fn config_path_is_absolute() {
        let path = config_path();
        assert!(path.is_absolute());
    }

    #[test]
    fn roundtrip_config_serialization() {
        let config = LauncherConfig {
            backend_url: Some("wss://test.com".to_string()),
            auth_token: Some("tok_test".to_string()),
            name: Some("test-launcher".to_string()),
            max_processes: Some(3),
        };
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: LauncherConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.backend_url, config.backend_url);
        assert_eq!(deserialized.auth_token, config.auth_token);
        assert_eq!(deserialized.name, config.name);
        assert_eq!(deserialized.max_processes, config.max_processes);
    }
}
