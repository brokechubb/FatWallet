use anyhow::Result;

pub type FatResult<T> = Result<T, FatError>;

#[derive(Debug, thiserror::Error)]
pub enum FatError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Decryption error: {0}")]
    Decryption(String),

    #[error("Keystore error: {0}")]
    Keystore(String),

    #[error("Wallet not found: {0}")]
    WalletNotFound(String),

    #[error("Wallet already exists: {0}")]
    WalletExists(String),

    #[error("Invalid private key: {0}")]
    InvalidPrivateKey(String),

    #[error("Invalid seed phrase: {0}")]
    InvalidSeedPhrase(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Price error: {0}")]
    Price(String),

    #[error("Address book error: {0}")]
    AddressBook(String),
}

impl FatError {
    pub fn encryption(msg: impl Into<String>) -> Self {
        Self::Encryption(msg.into())
    }

    pub fn decryption(msg: impl Into<String>) -> Self {
        Self::Decryption(msg.into())
    }

    pub fn keystore(msg: impl Into<String>) -> Self {
        Self::Keystore(msg.into())
    }

    pub fn rpc(msg: impl Into<String>) -> Self {
        Self::Rpc(msg.into())
    }

    pub fn price(msg: impl Into<String>) -> Self {
        Self::Price(msg.into())
    }
}