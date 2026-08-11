use anyhow::Result;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer as system_transfer;
use solana_transaction::Transaction;

use crate::error::FatError;

pub type ProgressFn = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

const SPL_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

/// Transfer SOL to a recipient.
pub async fn transfer_sol(
    rpc: &super::Rpc,
    sender: &Keypair,
    recipient: &str,
    amount_sol: f64,
    progress: Option<ProgressFn>,
) -> Result<String> {
    let recipient_pk: Pubkey = recipient
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid recipient address: {}", e)))?;

    let lamports: u64 = {
        let scaled = amount_sol * 1_000_000_000.0;
        if scaled < 0.0 || scaled > u64::MAX as f64 {
            return Err(FatError::rpc("Amount out of range").into());
        }
        scaled.round() as u64
    };
    if lamports == 0 {
        return Err(FatError::rpc("Amount is zero after rounding").into());
    }

    let mut instructions = Vec::new();

    // Priority fee — use getRecentPrioritizationFees for dynamic estimation
    let priority_fee = get_priority_fee_estimate(rpc, &sender.pubkey()).await.unwrap_or(5_000);
    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(200_000));
    instructions.push(ComputeBudgetInstruction::set_compute_unit_price(priority_fee));

    // SOL transfer
    instructions.push(system_transfer(
        &sender.pubkey(),
        &recipient_pk,
        lamports,
    ));

    send_and_confirm(rpc, sender, instructions, progress).await
}

/// Transfer an SPL token to a recipient. Auto-creates recipient ATA if needed.
pub async fn transfer_spl(
    rpc: &super::Rpc,
    sender: &Keypair,
    recipient: &str,
    mint: &str,
    amount: f64,
    decimals: u8,
    progress: Option<ProgressFn>,
) -> Result<String> {
    let recipient_pk: Pubkey = recipient
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid recipient address: {}", e)))?;

    let mint_pk: Pubkey = mint
        .parse()
        .map_err(|e| FatError::rpc(format!("Invalid mint address: {}", e)))?;

    // Determine which token program the mint uses
    let token_program: Pubkey = if is_token_2022(rpc, &mint_pk).await? {
        TOKEN_2022_PROGRAM.parse().unwrap()
    } else {
        SPL_TOKEN_PROGRAM.parse().unwrap()
    };

    // Derive ATAs
    let sender_ata = spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
        &sender.pubkey(),
        &mint_pk,
        &token_program,
    );
    let recipient_ata = spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
        &recipient_pk,
        &mint_pk,
        &token_program,
    );

    let atomic_amount: u64 = {
        let scaled = amount * 10f64.powi(decimals as i32);
        if scaled < 0.0 || scaled > u64::MAX as f64 {
            return Err(FatError::rpc("Amount out of range").into());
        }
        scaled.round() as u64
    };
    if atomic_amount == 0 {
        return Err(FatError::rpc("Amount is zero after rounding").into());
    }

    let mut instructions = Vec::new();

    // Priority fee — use getRecentPrioritizationFees for dynamic estimation
    let priority_fee = get_priority_fee_estimate(rpc, &sender.pubkey()).await.unwrap_or(5_000);
    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(200_000));
    instructions.push(ComputeBudgetInstruction::set_compute_unit_price(priority_fee));

    // Check if recipient ATA exists; if not, create it (idempotent)
    let recipient_ata_exists = rpc.client.get_account(&recipient_ata).await.is_ok();
    if !recipient_ata_exists {
        if let Some(ref p) = progress {
            p("Creating recipient token account...");
        }
        instructions.push(
            spl_associated_token_account_interface::instruction::create_associated_token_account_idempotent(
                &sender.pubkey(),
                &recipient_pk,
                &mint_pk,
                &token_program,
            ),
        );
    }

    // Token transfer instruction
    if let Some(ref p) = progress {
        p("Building transfer instruction...");
    }
    instructions.push(spl_token_interface::instruction::transfer(
        &token_program,
        &sender_ata,
        &recipient_ata,
        &sender.pubkey(),
        &[],
        atomic_amount,
    )?);

    send_and_confirm(rpc, sender, instructions, progress).await
}

/// Unified transfer: if mint == SOL_MINT, do SOL transfer; otherwise SPL.
pub async fn transfer(
    rpc: &super::Rpc,
    sender: &Keypair,
    recipient: &str,
    mint: &str,
    amount: f64,
    decimals: u8,
) -> Result<String> {
    transfer_with_progress(rpc, sender, recipient, mint, amount, decimals, None).await
}

/// Unified transfer with a progress callback for UI feedback.
pub async fn transfer_with_progress(
    rpc: &super::Rpc,
    sender: &Keypair,
    recipient: &str,
    mint: &str,
    amount: f64,
    decimals: u8,
    progress: Option<ProgressFn>,
) -> Result<String> {
    if mint == SOL_MINT {
        transfer_sol(rpc, sender, recipient, amount, progress).await
    } else {
        transfer_spl(rpc, sender, recipient, mint, amount, decimals, progress).await
    }
}

/// Fetch priority fee estimate using getRecentPrioritizationFees RPC method.
/// Falls back to a default of 5000 micro-lamports if the call fails.
async fn get_priority_fee_estimate(rpc: &super::Rpc, account: &Pubkey) -> Result<u64> {
    let fees = rpc
        .client
        .get_recent_prioritization_fees(&[*account])
        .await
        .map_err(|e| FatError::rpc(format!("getRecentPrioritizationFees failed: {}", e)))?;

    if fees.is_empty() {
        return Ok(5_000);
    }

    // Use the median fee from recent slots
    let mut sorted: Vec<u64> = fees.iter().map(|f| f.prioritization_fee).collect();
    sorted.sort();
    let mid = sorted.len() / 2;
    let median = sorted[mid];

    // Cap at 5M microLamports (0.005 SOL)
    Ok(median.min(5_000_000).max(1_000))
}

/// Check if a mint belongs to Token-2022 program.
async fn is_token_2022(rpc: &super::Rpc, mint: &Pubkey) -> Result<bool> {
    let token_2022: Pubkey = TOKEN_2022_PROGRAM.parse().unwrap();
    match rpc.client.get_account(mint).await {
        Ok(account) => Ok(account.owner == token_2022),
        Err(_) => Ok(false),
    }
}

/// Build, sign, send, and confirm a transaction.
/// Retries with a fresh blockhash if the original expires.
async fn send_and_confirm(
    rpc: &super::Rpc,
    sender: &Keypair,
    instructions: Vec<solana_instruction::Instruction>,
    progress: Option<ProgressFn>,
) -> Result<String> {
    let max_retries = 3;
    let mut last_err = String::new();

    for attempt in 0..max_retries {
        if let Some(ref p) = progress {
            p("Fetching blockhash...");
        }
        let blockhash = rpc
            .client
            .get_latest_blockhash()
            .await
            .map_err(|e| FatError::rpc(format!("getLatestBlockhash failed: {}", e)))?;

        let mut tx = Transaction::new_with_payer(&instructions, Some(&sender.pubkey()));
        tx.sign(&[sender], blockhash);

        if let Some(ref p) = progress {
            p("Submitting transaction...");
        }
        match rpc
            .client
            .send_and_confirm_transaction(&tx)
            .await
        {
            Ok(signature) => {
                if let Some(ref p) = progress {
                    p("Confirming on-chain...");
                }
                return Ok(signature.to_string());
            }
            Err(e) => {
                let err_str = e.to_string();
                last_err = err_str.clone();
                // Retry on blockhash expiry
                if err_str.contains("BlockhashNotFound") || err_str.contains("blockhash") {
                    eprintln!("Blockhash expired, retrying ({}/{})", attempt + 1, max_retries);
                    if let Some(ref p) = progress {
                        p(&format!("Blockhash expired — retry {}/{}", attempt + 1, max_retries));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                return Err(FatError::rpc(format!("sendAndConfirm failed: {}", e)).into());
            }
        }
    }

    Err(FatError::rpc(format!("Transaction failed after {} retries: {}", max_retries, last_err)).into())
}