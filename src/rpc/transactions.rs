use anyhow::Result;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use solana_rpc_client_types::config::RpcTransactionConfig;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiMessage, UiTransactionEncoding,
};

use crate::error::FatError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxHistoryEntry {
    pub signature: String,
    pub block_time: Option<i64>,
    pub slot: Option<u64>,
    pub err: Option<String>,
    pub fee: Option<u64>,
    pub description: String,
    pub amount: Option<f64>,
    pub token_symbol: String,
    pub token_mint: Option<String>,
    pub direction: TxDirection,
    pub counterparty: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum TxDirection {
    Incoming,
    Outgoing,
    Unknown,
}

/// Fetch transaction history for a wallet using standard RPC.
/// Fetches transaction details concurrently to reduce latency.
pub async fn fetch_transactions(
    rpc: &super::Rpc,
    pubkey: &str,
    limit: usize,
) -> Result<Vec<TxHistoryEntry>> {
    let pk: Pubkey = pubkey
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid pubkey: {}", e)))?;

    let signatures = rpc
        .client
        .get_signatures_for_address(&pk)
        .await
        .map_err(|e| FatError::rpc(format!("getSignaturesForAddress failed: {}", e)))?;

    let count = signatures.len().min(limit);
    let sigs: Vec<_> = signatures.into_iter().take(count).collect();

    // Spawn concurrent fetches for all non-failed transactions
    let mut handles = Vec::new();
    let mut failed_entries = Vec::new();

    for sig_info in sigs {
        let signature = sig_info.signature.clone();
        let block_time = sig_info.block_time;
        let err = sig_info.err.as_ref().map(|e| format!("{:?}", e));

        if err.is_some() {
            failed_entries.push((
                signature,
                block_time,
                Some(sig_info.slot),
                err,
            ));
            continue;
        }

        let rpc_url = rpc.url.clone();
        let wallet = pubkey.to_string();
        let sig = signature.clone();
        handles.push(tokio::spawn(async move {
            let rpc_client = solana_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
                rpc_url,
                solana_commitment_config::CommitmentConfig::confirmed(),
            );
            let sig_parsed: solana_signature::Signature = match sig.parse() {
                Ok(s) => s,
                Err(e) => {
                    return (signature, block_time, Some(sig_info.slot), Err(FatError::rpc(format!("Invalid signature: {}", e))), wallet);
                }
            };

            let tx_result = rpc_client
                .get_transaction_with_config(
                    &sig_parsed,
                    RpcTransactionConfig {
                        encoding: Some(UiTransactionEncoding::Json),
                        commitment: None,
                        max_supported_transaction_version: Some(0),
                    },
                )
                .await;

            (signature, block_time, Some(sig_info.slot), Ok(tx_result), wallet)
        }));
    }

    // Add failed transaction entries
    let mut entries = Vec::with_capacity(count);
    for (signature, block_time, slot, err) in failed_entries {
        entries.push(TxHistoryEntry {
            signature,
            block_time,
            slot,
            err,
            fee: None,
            description: "Failed transaction".to_string(),
            amount: None,
            token_symbol: String::new(),
            token_mint: None,
            direction: TxDirection::Unknown,
            counterparty: None,
        });
    }

    // Collect results from concurrent fetches
    let results = futures::future::join_all(handles).await;
    for result in results {
        match result {
            Ok((signature, block_time, slot, tx_result, wallet)) => {
                match tx_result {
                    Ok(Ok(encoded_tx)) => {
                        let entry = parse_transaction(&signature, &encoded_tx, &wallet);
                        entries.push(entry);
                    }
                    Ok(Err(e)) => {
                        entries.push(TxHistoryEntry {
                            signature,
                            block_time,
                            slot,
                            err: Some(format!("getTransaction failed: {}", e)),
                            fee: None,
                            description: "Unable to fetch details".to_string(),
                            amount: None,
                            token_symbol: String::new(),
                            token_mint: None,
                            direction: TxDirection::Unknown,
                            counterparty: None,
                        });
                    }
                    Err(e) => {
                        entries.push(TxHistoryEntry {
                            signature,
                            block_time,
                            slot,
                            err: Some(format!("Signature parse failed: {}", e)),
                            fee: None,
                            description: "Unable to fetch details".to_string(),
                            amount: None,
                            token_symbol: String::new(),
                            token_mint: None,
                            direction: TxDirection::Unknown,
                            counterparty: None,
                        });
                    }
                }
            }
            Err(e) => {
                eprintln!("Join error fetching transaction: {}", e);
            }
        }
    }

    // Sort by block_time descending (most recent first)
    entries.sort_by(|a, b| b.block_time.unwrap_or(0).cmp(&a.block_time.unwrap_or(0)));

    Ok(entries)
}

fn parse_transaction(
    signature: &str,
    encoded_tx: &EncodedConfirmedTransactionWithStatusMeta,
    wallet_pubkey: &str,
) -> TxHistoryEntry {
    let meta = &encoded_tx.transaction.meta;
    let enc_tx = &encoded_tx.transaction.transaction;

    let fee = meta.as_ref().map(|m| m.fee);

    // Extract account keys from the transaction
    let mut account_keys: Vec<String> = match &enc_tx {
        EncodedTransaction::Json(ui_tx) => match &ui_tx.message {
            UiMessage::Parsed(parsed) => {
                parsed.account_keys.iter().map(|a| a.pubkey.clone()).collect()
            }
            UiMessage::Raw(raw) => raw.account_keys.clone(),
        },
        _ => Vec::new(),
    };

    // For V0 transactions with address table lookups, append loaded addresses
    // from the meta. The order is: static keys, then writable LUT addresses,
    // then readonly LUT addresses — matching the pre/post balance array order.
    if let Some(m) = meta {
        let loaded: Option<&solana_transaction_status_client_types::UiLoadedAddresses> = m.loaded_addresses.as_ref().into();
        if let Some(la) = loaded {
            for addr in &la.writable {
                account_keys.push(addr.clone());
            }
            for addr in &la.readonly {
                account_keys.push(addr.clone());
            }
        }
    }

    let wallet = wallet_pubkey.to_string();

    // SOL balance change
    let sol_change = if let Some(meta) = meta {
        let mut change: Option<i64> = None;
        for (i, key) in account_keys.iter().enumerate() {
            if key == &wallet {
                let pre = meta.pre_balances.get(i).copied().unwrap_or(0);
                let post = meta.post_balances.get(i).copied().unwrap_or(0);
                change = Some(post as i64 - pre as i64);
                break;
            }
        }
        change
    } else {
        None
    };

    // Token balance changes
    let mut token_amount: Option<f64> = None;
    let mut token_symbol = String::new();
    let mut token_mint: Option<String> = None;
    let mut direction = TxDirection::Unknown;
    let mut counterparty: Option<String> = None;

    if let Some(meta) = meta {
        let pre_tokens = meta.pre_token_balances.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
        let post_tokens = meta.post_token_balances.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);

        // Find our wallet's token balance change by matching on owner field
        // (account_index points to the token account, not the wallet owner)
        for post in post_tokens {
            let owner: Option<&String> = post.owner.as_ref().into();
            if owner == Some(&wallet) {
                // Find the pre-balance for this same token account (match by account_index + mint)
                let pre_amount: f64 = pre_tokens
                    .iter()
                    .find(|p| p.account_index == post.account_index && p.mint == post.mint)
                    .map(|p| p.ui_token_amount.ui_amount.unwrap_or(0.0))
                    .unwrap_or(0.0);
                let post_amount: f64 = post.ui_token_amount.ui_amount.unwrap_or(0.0);
                let diff = post_amount - pre_amount;
                if diff.abs() > 0.0 {
                    token_amount = Some(diff.abs());
                    token_symbol = crate::rpc::balance::short_symbol_pub(&post.mint);
                    token_mint = Some(post.mint.clone());
                    direction = if diff > 0.0 {
                        TxDirection::Incoming
                    } else {
                        TxDirection::Outgoing
                    };
                    break;
                }
            }
        }

        // Also try matching by account_index -> account_keys -> wallet
        // (fallback for when owner field is not populated)
        if direction == TxDirection::Unknown {
            let wallet_index = account_keys.iter().position(|k| k == &wallet);
            if let Some(widx) = wallet_index {
                for post in post_tokens {
                    if post.account_index as usize == widx {
                        let pre_amount: f64 = pre_tokens
                            .iter()
                            .find(|p| p.account_index == post.account_index && p.mint == post.mint)
                            .map(|p| p.ui_token_amount.ui_amount.unwrap_or(0.0))
                            .unwrap_or(0.0);
                        let post_amount: f64 = post.ui_token_amount.ui_amount.unwrap_or(0.0);
                        let diff = post_amount - pre_amount;
                        if diff.abs() > 0.0 {
                            token_amount = Some(diff.abs());
                            token_symbol = crate::rpc::balance::short_symbol_pub(&post.mint);
                            token_mint = Some(post.mint.clone());
                            direction = if diff > 0.0 {
                                TxDirection::Incoming
                            } else {
                                TxDirection::Outgoing
                            };
                            break;
                        }
                    }
                }
            }
        }
    }

    // SOL direction from balance change
    if direction == TxDirection::Unknown {
        if let Some(sol_diff) = sol_change {
            if sol_diff > 0 {
                direction = TxDirection::Incoming;
                token_amount = Some(sol_diff as f64 / 1_000_000_000.0);
                token_symbol = "SOL".to_string();
            } else if sol_diff < 0 {
                direction = TxDirection::Outgoing;
                let outgoing = (-sol_diff) as f64 / 1_000_000_000.0;
                // Subtract fee from outgoing amount display
                let fee_sol = fee.map(|f| f as f64 / 1_000_000_000.0).unwrap_or(0.0);
                token_amount = Some((outgoing - fee_sol).max(0.0));
                token_symbol = "SOL".to_string();
            }
        }
    }

    // Find counterparty — the account whose balance changed in the opposite direction
    if let Some(meta) = meta {
        match direction {
            TxDirection::Outgoing => {
                // For outgoing: find an account whose SOL balance increased
                for (i, key) in account_keys.iter().enumerate() {
                    if key == &wallet {
                        continue;
                    }
                    let pre = meta.pre_balances.get(i).copied().unwrap_or(0);
                    let post = meta.post_balances.get(i).copied().unwrap_or(0);
                    if post > pre {
                        counterparty = Some(key.clone());
                        break;
                    }
                }
                // Also check token balances for SPL transfers
                if counterparty.is_none() {
                    let post_tokens = meta.post_token_balances.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
                    let pre_tokens = meta.pre_token_balances.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
                    for post in post_tokens {
                        let pre_amount: f64 = pre_tokens
                            .iter()
                            .find(|p| p.account_index == post.account_index && p.mint == post.mint)
                            .map(|p| p.ui_token_amount.ui_amount.unwrap_or(0.0))
                            .unwrap_or(0.0);
                        let post_amount: f64 = post.ui_token_amount.ui_amount.unwrap_or(0.0);
                        if post_amount > pre_amount {
                            if (post.account_index as usize) < account_keys.len() {
                                counterparty = Some(account_keys[post.account_index as usize].clone());
                                break;
                            }
                        }
                    }
                }
            }
            TxDirection::Incoming => {
                // For incoming: find an account whose SOL balance decreased
                for (i, key) in account_keys.iter().enumerate() {
                    if key == &wallet {
                        continue;
                    }
                    let pre = meta.pre_balances.get(i).copied().unwrap_or(0);
                    let post = meta.post_balances.get(i).copied().unwrap_or(0);
                    if post < pre {
                        counterparty = Some(key.clone());
                        break;
                    }
                }
                // Also check token balances for SPL transfers
                if counterparty.is_none() {
                    let post_tokens = meta.post_token_balances.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
                    let pre_tokens = meta.pre_token_balances.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
                    for post in post_tokens {
                        let pre_amount: f64 = pre_tokens
                            .iter()
                            .find(|p| p.account_index == post.account_index && p.mint == post.mint)
                            .map(|p| p.ui_token_amount.ui_amount.unwrap_or(0.0))
                            .unwrap_or(0.0);
                        let post_amount: f64 = post.ui_token_amount.ui_amount.unwrap_or(0.0);
                        if post_amount < pre_amount {
                            if (post.account_index as usize) < account_keys.len() {
                                counterparty = Some(account_keys[post.account_index as usize].clone());
                                break;
                            }
                        }
                    }
                }
            }
            TxDirection::Unknown => {
                // Fall back to looking at other signers
                if let EncodedTransaction::Json(ui_tx) = &enc_tx {
                    let num_signers = match &ui_tx.message {
                        UiMessage::Parsed(p) => p.account_keys.iter().take_while(|a| a.signer).count(),
                        UiMessage::Raw(r) => r.header.num_required_signatures as usize,
                    };
                    for (i, key) in account_keys.iter().enumerate() {
                        if i < num_signers && key != &wallet {
                            counterparty = Some(key.clone());
                            break;
                        }
                    }
                }
            }
        }
    }

    // Build description
    let arrow = match direction {
        TxDirection::Incoming => "+",
        TxDirection::Outgoing => "-",
        TxDirection::Unknown => " ",
    };
    let amount_str = token_amount
        .map(|a| format!("{}{:.6} {}", arrow, a, token_symbol))
        .unwrap_or_default();
    let counter_str = counterparty
        .as_ref()
        .map(|c| short_addr(c))
        .unwrap_or_default();
    let description = if amount_str.is_empty() {
        "Transaction".to_string()
    } else if counter_str.is_empty() {
        amount_str
    } else {
        format!("{} {}", amount_str, counter_str)
    };

    TxHistoryEntry {
        signature: signature.to_string(),
        block_time: encoded_tx.block_time,
        slot: Some(encoded_tx.slot),
        err: None,
        fee,
        description,
        amount: token_amount,
        token_symbol,
        token_mint,
        direction,
        counterparty,
    }
}

fn short_addr(addr: &str) -> String {
    if addr.len() > 8 {
        format!("{}...{}", &addr[..4], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}