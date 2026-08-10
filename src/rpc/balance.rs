use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use solana_rpc_client_types::request::TokenAccountsFilter;

use crate::error::FatError;

const SPL_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBalance {
    pub mint: String,
    pub symbol: String,
    pub amount: f64,
    pub decimals: u8,
    pub ui_amount_string: String,
    pub usd_price: Option<f64>,
    pub usd_value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletBalances {
    pub sol_balance: f64,
    pub sol_usd_price: Option<f64>,
    pub sol_usd_value: Option<f64>,
    pub tokens: Vec<TokenBalance>,
    pub total_usd_value: Option<f64>,
}

impl WalletBalances {
    pub fn count(&self) -> usize {
        1 + self.tokens.len()
    }
}

#[derive(Debug, Deserialize)]
struct TokenAccountInfo {
    mint: String,
    #[serde(rename = "tokenAmount")]
    token_amount: UiTokenAmount,
}

#[derive(Debug, Deserialize)]
struct ParsedInfo {
    #[serde(rename = "type")]
    account_type: String,
    info: TokenAccountInfo,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UiTokenAmount {
    amount: String,
    decimals: u8,
    #[serde(rename = "uiAmount")]
    ui_amount: Option<f64>,
    #[serde(rename = "uiAmountString")]
    ui_amount_string: String,
}

pub async fn fetch_balances(rpc: &super::Rpc, pubkey: &str) -> Result<WalletBalances> {
    let pk: Pubkey = pubkey
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid pubkey: {}", e)))?;

    // SOL balance (lamports -> SOL)
    let sol_lamports = rpc
        .client
        .get_balance(&pk)
        .await
        .map_err(|e| FatError::rpc(format!("getBalance failed: {}", e)))?;
    let sol_balance = sol_lamports as f64 / 1_000_000_000.0;

    // Token accounts via getTokenAccountsByOwner for both SPL Token and Token-2022
    // Fetch both programs concurrently to reduce latency
    let token_program: Pubkey = SPL_TOKEN_PROGRAM
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid token program id: {}", e)))?;
    let token_2022_program: Pubkey = TOKEN_2022_PROGRAM
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid token-2022 program id: {}", e)))?;

    let pk_clone = pk;
    let rpc_url = rpc.url.clone();
    let prog1 = token_program;
    let prog2 = token_2022_program;

    let (accounts1, accounts2) = tokio::join!(
        fetch_token_accounts(&rpc_url, pk_clone, prog1),
        fetch_token_accounts(&rpc_url, pk_clone, prog2),
    );

    let mut tokens = Vec::new();

    for accounts in [accounts1, accounts2] {
        match accounts {
            Ok(rpc_accounts) => {
                for keyed_account in rpc_accounts {
                    if let solana_account_decoder_client_types::UiAccountData::Json(ref parsed) =
                        keyed_account.account.data
                    {
                        if parsed.program != "spl-token" && parsed.program != "spl-token-2022" {
                            continue;
                        }
                        if let Ok(parsed_info) =
                            serde_json::from_value::<ParsedInfo>(parsed.parsed.clone())
                        {
                            if parsed_info.account_type != "account" {
                                continue;
                            }
                            let info = parsed_info.info;
                            if info.token_amount.ui_amount.unwrap_or(0.0) == 0.0 {
                                continue;
                            }
                            tokens.push(TokenBalance {
                                mint: info.mint.clone(),
                                symbol: short_symbol(&info.mint),
                                amount: info.token_amount.ui_amount.unwrap_or(0.0),
                                decimals: info.token_amount.decimals,
                                ui_amount_string: info.token_amount.ui_amount_string,
                                usd_price: None,
                                usd_value: None,
                            });
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: getTokenAccountsByOwner failed: {}",
                    e
                );
            }
        }
    }

    // Deduplicate by mint (keep highest balance if duplicates from both programs)
    tokens.sort_by(|a, b| b.amount.partial_cmp(&a.amount).unwrap_or(std::cmp::Ordering::Equal));
    tokens.dedup_by(|a, b| a.mint == b.mint);

    Ok(WalletBalances {
        sol_balance,
        sol_usd_price: None,
        sol_usd_value: None,
        tokens,
        total_usd_value: None,
    })
}

pub fn enrich_with_prices(balances: &mut WalletBalances, prices: &HashMap<String, f64>) {
    if let Some(sol_price) = prices.get(SOL_MINT) {
        balances.sol_usd_price = Some(*sol_price);
        balances.sol_usd_value = Some(balances.sol_balance * sol_price);
    }

    for token in &mut balances.tokens {
        if let Some(price) = prices.get(&token.mint) {
            token.usd_price = Some(*price);
            token.usd_value = Some(token.amount * price);
        }
    }

    let mut total = 0.0;
    let mut has_any = false;
    if let Some(sol_val) = balances.sol_usd_value {
        total += sol_val;
        has_any = true;
    }
    for token in &balances.tokens {
        if let Some(val) = token.usd_value {
            total += val;
            has_any = true;
        }
    }
    if has_any {
        balances.total_usd_value = Some(total);
    }
}

pub const KNOWN_MINTS: &[(&str, &str)] = &[
    (SOL_MINT, "SOL"),
    ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "USDC"),
    ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", "USDT"),
];

pub fn short_symbol(mint: &str) -> String {
    short_symbol_pub(mint)
}

pub fn short_symbol_pub(mint: &str) -> String {
    for (m, sym) in KNOWN_MINTS {
        if mint == *m {
            return sym.to_string();
        }
    }
    // Try the token metadata cache
    if let Ok(cache) = crate::token_metadata::TokenCache::load() {
        if let Some(sym) = cache.get_symbol(mint) {
            return sym;
        }
    }
    if mint.len() > 8 {
        format!("{}...{}", &mint[..4], &mint[mint.len() - 4..])
    } else {
        mint.to_string()
    }
}

/// Fetch token accounts for a specific program ID concurrently.
type TokenAccountsResult = std::result::Result<
    Vec<solana_rpc_client_types::response::RpcKeyedAccount>,
    solana_client::client_error::ClientError,
>;

async fn fetch_token_accounts(
    rpc_url: &str,
    owner: Pubkey,
    program_id: Pubkey,
) -> TokenAccountsResult {
    let client = solana_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
        rpc_url.to_string(),
        solana_commitment_config::CommitmentConfig::confirmed(),
    );
    client
        .get_token_accounts_by_owner(&owner, TokenAccountsFilter::ProgramId(program_id))
        .await
}