use anyhow::Result;
use solana_keypair::Keypair;
use solana_signer::Signer;
use zeroize::Zeroize;

use crate::error::FatError;

pub mod keystore;
pub mod keypair;
pub mod types;

pub use keystore::{decrypt_keypair, encrypt_keypair, KeystoreFile};
pub use types::WalletInfo;

pub struct UnlockedKeypair {
    pub keypair: Keypair,
    pub pubkey: String,
    pub label: String,
}

impl Drop for UnlockedKeypair {
    fn drop(&mut self) {
        let bytes = self.keypair.to_bytes();
        let mut arr: [u8; 64] = bytes;
        arr.zeroize();
    }
}

/// Create a new wallet: generate mnemonic, derive keypair, encrypt and save.
pub fn create_wallet(label: &str, passphrase: &str) -> Result<(WalletInfo, String)> {
    crate::config::Config::ensure_dirs()?;

    let mnemonic = keypair::generate_mnemonic()?;
    let keypair = keypair::keypair_from_seed_phrase(&mnemonic, "", 0, 0)?;
    let pubkey = keypair.pubkey().to_string();

    let path = KeystoreFile::path_for(&pubkey)?;
    if path.exists() {
        return Err(FatError::WalletExists(pubkey).into());
    }

    let keypair_bytes = keypair.to_bytes();
    let keystore = encrypt_keypair(&keypair_bytes, passphrase, &pubkey, label)?;
    keystore.save(&path)?;

    Ok((WalletInfo::from(&keystore), mnemonic))
}

/// Import a wallet from a seed phrase, encrypt and save.
pub fn import_from_seed_phrase(
    seed_phrase: &str,
    passphrase: &str,
    label: &str,
    account: u32,
    change: u32,
) -> Result<WalletInfo> {
    crate::config::Config::ensure_dirs()?;

    let keypair = keypair::keypair_from_seed_phrase(seed_phrase, "", account, change)?;
    let pubkey = keypair.pubkey().to_string();

    let path = KeystoreFile::path_for(&pubkey)?;
    if path.exists() {
        return Err(FatError::WalletExists(pubkey).into());
    }

    let keypair_bytes = keypair.to_bytes();
    let keystore = encrypt_keypair(&keypair_bytes, passphrase, &pubkey, label)?;
    keystore.save(&path)?;

    Ok(WalletInfo::from(&keystore))
}

/// Import a wallet from a base58 private key, encrypt and save.
pub fn import_from_private_key(private_key: &str, passphrase: &str, label: &str) -> Result<WalletInfo> {
    crate::config::Config::ensure_dirs()?;

    let keypair = keypair::keypair_from_base58(private_key)?;
    let pubkey = keypair.pubkey().to_string();

    let path = KeystoreFile::path_for(&pubkey)?;
    if path.exists() {
        return Err(FatError::WalletExists(pubkey).into());
    }

    let keypair_bytes = keypair.to_bytes();
    let keystore = encrypt_keypair(&keypair_bytes, passphrase, &pubkey, label)?;
    keystore.save(&path)?;

    Ok(WalletInfo::from(&keystore))
}

/// List all wallets (metadata only, no decryption).
pub fn list_wallets() -> Result<Vec<WalletInfo>> {
    let keystores = KeystoreFile::list_all()?;
    Ok(keystores.iter().map(WalletInfo::from).collect())
}

/// Remove a wallet by deleting its encrypted keystore file.
pub fn remove_wallet(pubkey: &str) -> Result<()> {
    let path = KeystoreFile::path_for(pubkey)?;
    if !path.exists() {
        return Err(FatError::WalletNotFound(pubkey.to_string()).into());
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

/// Unlock a wallet by pubkey, decrypting the keypair with the passphrase.
pub fn unlock_wallet(pubkey: &str, passphrase: &str) -> Result<UnlockedKeypair> {
    let path = KeystoreFile::path_for(pubkey)?;
    if !path.exists() {
        return Err(FatError::WalletNotFound(pubkey.to_string()).into());
    }

    let keystore = KeystoreFile::load(&path)?;
    let keypair_bytes = decrypt_keypair(&keystore, passphrase)?;

    let keypair = Keypair::try_from(&keypair_bytes[..])
        .map_err(|e| FatError::decryption(format!("Failed to reconstruct keypair: {}", e)))?;

    Ok(UnlockedKeypair {
        keypair,
        pubkey: keystore.pubkey,
        label: keystore.label,
    })
}

/// Prompt for a passphrase on stdin (hidden input).
pub fn prompt_passphrase(confirm: bool) -> Result<String> {
    let passphrase = rpassword::prompt_password("Passphrase: ")?;
    if passphrase.is_empty() {
        return Err(FatError::keystore("Passphrase cannot be empty").into());
    }
    if confirm {
        let confirm = rpassword::prompt_password("Confirm passphrase: ")?;
        if passphrase != confirm {
            return Err(FatError::keystore("Passphrases do not match").into());
        }
    }
    Ok(passphrase)
}

/// Prompt for a passphrase without confirmation (for unlock).
pub fn prompt_passphrase_unlock() -> Result<String> {
    let passphrase = rpassword::prompt_password("Passphrase: ")?;
    Ok(passphrase)
}