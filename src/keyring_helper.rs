use anyhow::Result;

use crate::error::FatError;

const KEYRING_SERVICE: &str = "fatwallet";
const KEYRING_USER: &str = "fatwallet_passphrase";

/// Try to retrieve the passphrase from the OS keyring.
/// Returns Ok(Some(passphrase)) if found, Ok(None) if not set, Err if keyring is unavailable.
pub fn get_passphrase() -> Result<Option<String>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| FatError::Config(format!("Keyring access failed: {}", e)))?;

    match entry.get_password() {
        Ok(passphrase) => Ok(Some(passphrase)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(FatError::Config(format!("Keyring read failed: {}", e)).into()),
    }
}

/// Store the passphrase in the OS keyring for future auto-unlock.
pub fn set_passphrase(passphrase: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| FatError::Config(format!("Keyring access failed: {}", e)))?;

    entry
        .set_password(passphrase)
        .map_err(|e| FatError::Config(format!("Keyring write failed: {}", e)).into())
}

/// Remove the passphrase from the OS keyring.
pub fn delete_passphrase() -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| FatError::Config(format!("Keyring access failed: {}", e)))?;

    match entry.delete_credential() {
        Ok(_) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(FatError::Config(format!("Keyring delete failed: {}", e)).into()),
    }
}

/// Check if a passphrase is stored in the keyring.
pub fn has_passphrase() -> bool {
    get_passphrase().unwrap_or(None).is_some()
}