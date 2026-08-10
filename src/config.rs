use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub rpc_url: String,
    #[serde(default)]
    pub jupiter_api_url: String,
    #[serde(default)]
    pub jupiter_api_key: String,
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval_secs: u64,
}

fn default_refresh_interval() -> u64 {
    30
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            jupiter_api_url: "https://api.jup.ag/price/v3".to_string(),
            jupiter_api_key: String::new(),
            refresh_interval_secs: 30,
        }
    }
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("fatwallet");
        Ok(base)
    }

    pub fn wallets_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("wallets"))
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn contacts_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("contacts.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config: {}", path.display()))?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create config dir: {}", dir.display()))?;
        let path = Self::config_path()?;
        let toml_str = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;
        Self::write_secure(&path, toml_str.as_bytes())?;
        Ok(())
    }

    pub fn ensure_dirs() -> Result<()> {
        let config_dir = Self::config_dir()?;
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("Failed to create config dir: {}", config_dir.display()))?;
        let wallets_dir = Self::wallets_dir()?;
        fs::create_dir_all(&wallets_dir)
            .with_context(|| format!("Failed to create wallets dir: {}", wallets_dir.display()))?;
        Ok(())
    }

    fn write_secure(path: &Path, data: &[u8]) -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("Failed to open for write: {}", path.display()))?;
        file.write_all(data)
            .with_context(|| format!("Failed to write: {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(path, perms)?;
        }
        Ok(())
    }
}