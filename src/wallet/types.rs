use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletInfo {
    pub pubkey: String,
    pub label: String,
    pub created_at: i64,
}

impl From<&super::keystore::KeystoreFile> for WalletInfo {
    fn from(ks: &super::keystore::KeystoreFile) -> Self {
        Self {
            pubkey: ks.pubkey.clone(),
            label: ks.label.clone(),
            created_at: ks.created_at,
        }
    }
}