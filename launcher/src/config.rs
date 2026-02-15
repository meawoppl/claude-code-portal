use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default)]
pub struct LauncherConfig {
    pub backend_url: Option<String>,
    pub auth_token: Option<String>,
    pub name: Option<String>,
    pub proxy_path: Option<String>,
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
