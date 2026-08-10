use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::FatError;

const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

#[derive(Debug, Deserialize)]
struct JupiterPriceResponse {
    #[serde(flatten)]
    prices: HashMap<String, JupiterPriceData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JupiterPriceData {
    #[serde(rename = "usdPrice")]
    usd_price: Option<f64>,
    #[serde(default)]
    decimals: Option<u8>,
}

pub struct PriceService {
    pub api_url: String,
    pub api_key: Option<String>,
    cache: Arc<RwLock<Option<PriceCache>>>,
    cache_ttl: Duration,
}

struct PriceCache {
    prices: HashMap<String, f64>,
    fetched_at: Instant,
}

impl PriceService {
    pub fn new(api_url: &str, api_key: Option<String>) -> Self {
        Self {
            api_url: api_url.to_string(),
            api_key,
            cache: Arc::new(RwLock::new(None)),
            cache_ttl: Duration::from_secs(60),
        }
    }

    pub fn from_config() -> Result<Self> {
        let config = crate::config::Config::load()?;
        let api_key = if config.jupiter_api_key.is_empty() {
            None
        } else {
            Some(config.jupiter_api_key.clone())
        };
        Ok(Self::new(&config.jupiter_api_url, api_key))
    }

    /// Fetch prices for a list of mint addresses. Always includes SOL.
    pub async fn get_prices(&self, mints: &[String]) -> Result<HashMap<String, f64>> {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(ref cached) = *cache {
                if cached.fetched_at.elapsed() < self.cache_ttl {
                    return Ok(cached.prices.clone());
                }
            }
        }

        // Build mint list (always include SOL)
        let mut all_mints: Vec<String> = vec![SOL_MINT.to_string()];
        for m in mints {
            if m != SOL_MINT && !all_mints.contains(m) {
                all_mints.push(m.clone());
            }
        }

        let ids = all_mints.join(",");
        let url = format!("{}?ids={}", self.api_url, ids);

        let client = reqwest::Client::new();
        let mut req = client.get(&url);
        if let Some(ref key) = self.api_key {
            req = req.header("x-api-key", key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| FatError::price(format!("Jupiter API request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(FatError::price(format!(
                "Jupiter API returned HTTP {}",
                resp.status()
            ))
            .into());
        }

        let body: JupiterPriceResponse = resp
            .json()
            .await
            .map_err(|e| FatError::price(format!("Jupiter API parse failed: {}", e)))?;

        let mut prices = HashMap::new();
        for (mint, data) in body.prices {
            if let Some(price) = data.usd_price {
                if price > 0.0 {
                    prices.insert(mint, price);
                }
            }
        }

        // Update cache
        {
            let mut cache = self.cache.write().await;
            *cache = Some(PriceCache {
                prices: prices.clone(),
                fetched_at: Instant::now(),
            });
        }

        Ok(prices)
    }

    /// Get price for a single mint.
    pub async fn get_price(&self, mint: &str) -> Result<Option<f64>> {
        let prices = self.get_prices(&[mint.to_string()]).await?;
        Ok(prices.get(mint).copied())
    }
}