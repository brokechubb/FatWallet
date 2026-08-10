pub mod balance;
pub mod swap;
pub mod transactions;
pub mod transfer;

use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;

use crate::error::FatError;

pub struct Rpc {
    pub client: RpcClient,
    pub url: String,
}

impl Rpc {
    pub fn new(rpc_url: &str) -> Self {
        let client = RpcClient::new_with_commitment(
            rpc_url.to_string(),
            CommitmentConfig::confirmed(),
        );
        Self { client, url: rpc_url.to_string() }
    }

    pub fn from_config() -> Result<Self> {
        let config = crate::config::Config::load()?;
        if config.rpc_url.is_empty() {
            return Err(FatError::Config("RPC URL not configured. Run 'fatwallet config set-rpc <url>' or edit ~/.config/fatwallet/config.toml".to_string()).into());
        }
        Ok(Self::new(&config.rpc_url))
    }
}