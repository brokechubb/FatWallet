use anyhow::{Context, Result};
use bip39::{Language, Mnemonic};
use solana_keypair::Keypair;
use zeroize::Zeroize;

use crate::error::FatError;

/// Generate a new 12-word BIP39 mnemonic.
pub fn generate_mnemonic() -> Result<String> {
    let mnemonic = Mnemonic::generate_in(Language::English, 12)
        .context("Failed to generate mnemonic")?;
    Ok(mnemonic.to_string())
}

/// Derive a Solana keypair from a BIP39 seed phrase using BIP44 m/44'/501'/0'/0'.
pub fn keypair_from_seed_phrase(seed_phrase: &str, passphrase: &str, account: u32, change: u32) -> Result<Keypair> {
    let mnemonic = Mnemonic::parse_normalized(seed_phrase)
        .map_err(|e| FatError::InvalidSeedPhrase(format!("Invalid mnemonic: {}", e)))?;

    let seed = mnemonic.to_seed(passphrase);

    let dp = solana_derivation_path::DerivationPath::new_bip44(Some(account), Some(change));
    let keypair = solana_keypair::seed_derivable::keypair_from_seed_and_derivation_path(&seed, Some(dp))
        .map_err(|e| FatError::InvalidSeedPhrase(format!("BIP44 derivation failed: {}", e)))?;

    Ok(keypair)
}

/// Import a keypair from a base58-encoded 64-byte private key (Phantom export format).
pub fn keypair_from_base58(private_key: &str) -> Result<Keypair> {
    let bytes = bs58::decode(private_key)
        .into_vec()
        .map_err(|e| FatError::InvalidPrivateKey(format!("Base58 decode failed: {}", e)))?;

    if bytes.len() != 64 {
        return Err(FatError::InvalidPrivateKey(format!(
            "Expected 64 bytes, got {}",
            bytes.len()
        )).into());
    }

    let mut arr = [0u8; 64];
    arr.copy_from_slice(&bytes);
    let keypair = Keypair::try_from(&arr[..])
        .map_err(|e| FatError::InvalidPrivateKey(format!("Invalid keypair: {}", e)))?;
    arr.zeroize();
    Ok(keypair)
}

/// Generate a random keypair (no mnemonic).
pub fn generate_keypair() -> Keypair {
    Keypair::new()
}