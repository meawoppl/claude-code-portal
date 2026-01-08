use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub sessions: HashMap<String, SessionAuth>,

    #[serde(default)]
    pub preferences: Preferences,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAuth {
    pub user_id: String,
    pub auth_token: String,
    pub user_email: Option<String>,
    pub last_used: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Preferences {
    #[serde(default)]
    pub default_backend_url: Option<String>,

    #[serde(default)]
    pub auto_open_browser: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            preferences: Preferences::default(),
        }
    }
}

impl ProxyConfig {
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = directories::ProjectDirs::from("com", "cc-proxy", "cc-proxy")
            .context("Failed to determine config directory")?
            .config_dir()
            .to_path_buf();

        Ok(config_dir.join("config.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path).context("Failed to read config file")?;

        let config: Self =
            serde_json::from_str(&contents).context("Failed to parse config file")?;

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let contents = serde_json::to_string_pretty(self).context("Failed to serialize config")?;

        fs::write(&path, contents).context("Failed to write config file")?;

        Ok(())
    }

    pub fn get_session_auth(&self, working_dir: &str) -> Option<&SessionAuth> {
        self.sessions.get(working_dir)
    }

    pub fn set_session_auth(&mut self, working_dir: String, auth: SessionAuth) {
        self.sessions.insert(working_dir, auth);
    }

    pub fn remove_session_auth(&mut self, working_dir: &str) -> Option<SessionAuth> {
        self.sessions.remove(working_dir)
    }
}
