use anyhow::Result;
use base64::Engine;
use serde::Deserialize;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;

use crate::error::FatError;

const JUPITER_ULTRA_ORDER_URL: &str = "https://lite-api.jup.ag/ultra/v1/order";
const JUPITER_ULTRA_EXECUTE_URL: &str = "https://lite-api.jup.ag/ultra/v1/execute";

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UltraOrderResponse {
    transaction: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(default)]
    error_code: Option<serde_json::Value>,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default, rename = "outAmount")]
    out_amount: Option<String>,
    #[serde(default, rename = "inAmount")]
    in_amount: Option<String>,
    #[serde(default, rename = "swapMode")]
    swap_mode: Option<String>,
    #[serde(default)]
    gasless: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct UltraExecuteResponse {
    #[serde(rename = "signature")]
    txid: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    code: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

/// Token decimals for known tokens.
pub fn token_decimals(mint: &str) -> u8 {
    match mint {
        "So11111111111111111111111111111111111111112" => 9,
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => 6,
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => 6,
        _ => 9,
    }
}

/// Execute a gasless swap via Jupiter Ultra API.
/// 1. Get order (base64 tx) from Jupiter
/// 2. Deserialize, sign with user keypair
/// 3. Submit signed tx to Jupiter executor
/// 4. Confirm tx landed
pub async fn gasless_swap(
    rpc: &super::Rpc,
    keypair: &Keypair,
    input_mint: &str,
    output_mint: &str,
    amount: f64,
) -> Result<String> {
    if amount <= 0.0 {
        return Err(FatError::rpc("Swap amount must be positive").into());
    }

    let decimals = token_decimals(input_mint);
    // Use u128 for precision: convert to atomic units without f64 overflow
    let amount_atomic: u64 = {
        let scaled = amount * 10f64.powi(decimals as i32);
        if scaled < 0.0 || scaled > u64::MAX as f64 {
            return Err(FatError::rpc("Swap amount out of range").into());
        }
        scaled.round() as u64
    };
    if amount_atomic == 0 {
        return Err(FatError::rpc("Swap amount rounds to zero").into());
    }

    // 1. Get order from Jupiter Ultra
    let url = format!(
        "{}?inputMint={}&outputMint={}&amount={}&taker={}",
        JUPITER_ULTRA_ORDER_URL,
        input_mint,
        output_mint,
        amount_atomic,
        keypair.pubkey()
    );

    let client = reqwest::Client::new();
    let config = crate::config::Config::load().unwrap_or_default();
    let mut req = client.get(&url);
    if !config.jupiter_api_key.is_empty() {
        req = req.header("x-api-key", &config.jupiter_api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| FatError::rpc(format!("Jupiter order request failed: {}", e)))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(FatError::rpc(format!("Jupiter order HTTP error: {}", text)).into());
    }

    let order: UltraOrderResponse = resp
        .json()
        .await
        .map_err(|e| FatError::rpc(format!("Jupiter order parse failed: {}", e)))?;

    // Check for errors — Jupiter returns errorCode/errorMessage when simulation fails
    if order.transaction.is_none() {
        let msg = order.error_message.unwrap_or_default();
        let code = order.error_code.as_ref().map(|c| c.to_string()).unwrap_or_default();
        return Err(FatError::rpc(format!(
            "Jupiter returned no transaction (code: {}): {}",
            code, msg
        ))
        .into());
    }

    // Validate the quote — reject if outAmount is suspiciously low
    if let Some(ref out) = order.out_amount {
        let out_atomic: u128 = out.parse().unwrap_or(0);

        if out_atomic == 0 {
            return Err(FatError::rpc("Jupiter quoted zero output — refusing to swap").into());
        }
    }

    let tx_base64 = order.transaction.unwrap();
    let request_id = order.request_id.unwrap_or_default();

    // 2. Deserialize the versioned transaction from base64
    let tx_bytes = base64::engine::general_purpose::STANDARD
        .decode(&tx_base64)
        .map_err(|e| FatError::rpc(format!("Base64 decode failed: {}", e)))?;

    let tx: VersionedTransaction = bincode::deserialize(&tx_bytes)
        .map_err(|e| FatError::rpc(format!("Transaction deserialize failed: {}", e)))?;

    // 3. Sign the transaction
    let mut signed_tx = tx;
    let message_bytes = signed_tx.message.serialize();
    let sig = keypair.try_sign_message(&message_bytes)?;
    if signed_tx.signatures.is_empty() {
        signed_tx.signatures.push(sig);
    } else {
        signed_tx.signatures[0] = sig;
    }

    // 4. Serialize signed tx and submit to Jupiter Ultra execute
    let signed_tx_bytes = bincode::serialize(&signed_tx)
        .map_err(|e| FatError::rpc(format!("Transaction serialize failed: {}", e)))?;
    let signed_b64 = base64::engine::general_purpose::STANDARD.encode(&signed_tx_bytes);

    let execute_body = serde_json::json!({
        "signedTransaction": signed_b64,
        "requestId": request_id,
    });

    let mut exec_req = client
        .post(JUPITER_ULTRA_EXECUTE_URL)
        .header("Content-Type", "application/json");
    if !config.jupiter_api_key.is_empty() {
        exec_req = exec_req.header("x-api-key", &config.jupiter_api_key);
    }
    let resp = exec_req
        .json(&execute_body)
        .send()
        .await
        .map_err(|e| FatError::rpc(format!("Jupiter execute request failed: {}", e)))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(FatError::rpc(format!("Jupiter execute HTTP error: {}", text)).into());
    }

    let result: UltraExecuteResponse = resp
        .json()
        .await
        .map_err(|e| FatError::rpc(format!("Jupiter execute parse failed: {}", e)))?;

    if result.status.as_deref() == Some("Failed") || result.error.is_some() {
        let code_str = result.code.as_ref().map(|c| c.to_string());
        return Err(FatError::rpc(format!(
            "Swap failed: {}",
            result.error.or(code_str).unwrap_or("unknown error".to_string())
        ))
        .into());
    }

    let txid = result
        .txid
        .ok_or_else(|| FatError::rpc("No transaction signature returned".to_string()))?;

    // 5. Confirm the transaction — poll with shorter interval and fewer retries
    let sig: solana_signature::Signature = txid
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid signature: {}", e)))?;

    for _ in 0..15 {
        match rpc.client.get_signature_status(&sig).await {
            Ok(Some(Ok(()))) => return Ok(txid),
            Ok(Some(Err(e))) => {
                return Err(FatError::rpc(format!("Transaction failed on-chain: {:?}", e)).into());
            }
            Ok(None) => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }

    // Return the txid even if confirmation times out — it may still land
    Ok(txid)
}