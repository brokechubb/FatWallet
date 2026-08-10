use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Config;

const KNOWN_TOKENS: &[(&str, &str, u8)] = &[
    ("So11111111111111111111111111111111111111112", "SOL", 9),
    ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "USDC", 6),
    ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", "USDT", 6),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenMeta {
    symbol: String,
    decimals: u8,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct TokenCache {
    tokens: HashMap<String, TokenMeta>,
}

impl TokenCache {
    fn cache_path() -> Result<PathBuf> {
        Ok(Config::config_dir()?.join("token_cache.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::cache_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path)?;
        let cache: TokenCache = serde_json::from_str(&contents)?;
        Ok(cache)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::cache_path()?;
        let json = serde_json::to_string_pretty(self)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.write_all(json.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms)?;
        }
        Ok(())
    }

    pub fn get_symbol(&self, mint: &str) -> Option<String> {
        for (m, sym, _) in KNOWN_TOKENS {
            if mint == *m {
                return Some(sym.to_string());
            }
        }
        self.tokens.get(mint).map(|m| m.symbol.clone())
    }

    pub fn get_decimals(&self, mint: &str) -> Option<u8> {
        for (m, _, dec) in KNOWN_TOKENS {
            if mint == *m {
                return Some(*dec);
            }
        }
        self.tokens.get(mint).map(|m| m.decimals)
    }

    pub fn insert(&mut self, mint: &str, symbol: &str, decimals: u8) {
        self.tokens.insert(
            mint.to_string(),
            TokenMeta {
                symbol: symbol.to_string(),
                decimals,
            },
        );
    }
}

/// Fetch token metadata for unknown mints via Helius DAS getAssetBatch.
/// Caches results to disk for future use.
pub async fn fetch_and_cache_metadata(mints: &[String]) -> Result<TokenCache> {
    let mut cache = TokenCache::load()?;

    // Filter to mints we don't already know
    let unknown: Vec<&String> = mints
        .iter()
        .filter(|m| cache.get_symbol(m).is_none())
        .collect();

    if unknown.is_empty() {
        return Ok(cache);
    }

    let config = Config::load()?;
    if config.rpc_url.is_empty() {
        return Ok(cache);
    }

    // Use Helius DAS getAssetBatch
    let client = reqwest::Client::new();
    let ids: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "getAssetBatch",
        "params": {
            "ids": ids,
            "options": {
                "showFungible": true
            }
        }
    });

    let resp = client
        .post(&config.rpc_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;

    if let Ok(resp) = resp {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                // Result can be an array (getAssetBatch) or object (getAsset)
                if let Some(items) = json.get("result").and_then(|r| r.as_array()) {
                    for item in items {
                        parse_and_cache_asset(item, &mut cache);
                    }
                    let _ = cache.save();
                } else if let Some(result) = json.get("result") {
                    // Single getAsset response
                    parse_and_cache_asset(result, &mut cache);
                    let _ = cache.save();
                } else if let Some(error) = json.get("error") {
                    eprintln!("DAS getAssetBatch error: {}", error);
                }
            }
        } else {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            eprintln!("DAS getAssetBatch HTTP {}: {}", status, text);
        }
    }

    Ok(cache)
}

/// Parse a single asset response and add to cache if valid.
fn parse_and_cache_asset(item: &serde_json::Value, cache: &mut TokenCache) {
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");

    // Try token_info.symbol first (most reliable for fungible tokens)
    let symbol = item
        .get("token_info")
        .and_then(|t| t.get("symbol"))
        .and_then(|s| s.as_str())
        .or_else(|| {
            // Fallback to content.metadata.symbol
            item.get("content")
                .and_then(|c| c.get("metadata"))
                .and_then(|m| m.get("symbol"))
                .and_then(|s| s.as_str())
        })
        .unwrap_or("");

    let decimals = item
        .get("token_info")
        .and_then(|t| t.get("decimals"))
        .and_then(|d| {
            // decimals can be a number or string
            d.as_u64()
                .map(|v| v as u8)
                .or_else(|| d.as_str().and_then(|s| s.parse::<u8>().ok()))
        })
        .unwrap_or(9);

    if !id.is_empty() && !symbol.is_empty() {
        cache.insert(id, symbol, decimals);
    }
}

/// Get a display symbol for a mint, using cache or falling back to truncated mint.
pub fn display_symbol(mint: &str, cache: &TokenCache) -> String {
    if let Some(sym) = cache.get_symbol(mint) {
        return sym;
    }
    if mint.len() > 8 {
        format!("{}...{}", &mint[..4], &mint[mint.len() - 4..])
    } else {
        mint.to_string()
    }
}