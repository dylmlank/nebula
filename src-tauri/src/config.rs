use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub model: String,
    pub speed: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleConfig {
    #[serde(default)]
    pub search_api_key: String,
    #[serde(default)]
    pub search_cx: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
}

impl Default for GoogleConfig {
    fn default() -> Self {
        Self {
            search_api_key: String::new(),
            search_cx: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub fast_model: String,
    pub fallback_models: Vec<String>,
    pub max_tokens: u32,
    pub fast_max_tokens: u32,
    pub vault_path: String,
    pub voice: VoiceConfig,
    pub theme: String,
    #[serde(default)]
    pub google: GoogleConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            base_url: std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into()),
            model: "us.anthropic.claude-sonnet-4-6".into(),
            fast_model: "us.anthropic.claude-haiku-4-5-20251001-v1:0".into(),
            fallback_models: vec![
                "anthropic.claude-sonnet-4-6".into(),
                "us.anthropic.claude-sonnet-4-5-20250929-v1:0".into(),
            ],
            max_tokens: 4096,
            fast_max_tokens: 2048,
            vault_path: "~/Documents/vault/Nebula".into(),
            voice: VoiceConfig {
                enabled: true,
                model: "/home/dylan/.local/share/piper-voices/en_US-ryan-high.onnx".into(),
                speed: 1.25,
            },
            theme: "dark".into(),
            google: GoogleConfig::default(),
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("nebula");
        config_dir.join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();

        let mut config = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        };

        // Env var overrides
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            config.api_key = key;
        }
        if let Ok(url) = std::env::var("ANTHROPIC_BASE_URL") {
            config.base_url = url;
        }
        if let Ok(vault) = std::env::var("NEBULA_VAULT") {
            config.vault_path = vault;
        }

        config
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn vault_path_expanded(&self) -> PathBuf {
        let expanded = shellexpand::tilde(&self.vault_path);
        PathBuf::from(expanded.as_ref())
    }
}
