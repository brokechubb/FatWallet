#![allow(deprecated)]
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::FatError;

const KEYSTORE_VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const ARGON2_M_COST: u32 = 65536; // 64 MiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub salt: Vec<u8>,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystoreFile {
    pub version: u32,
    pub kdf: String,
    pub kdf_params: KdfParams,
    pub cipher: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub pubkey: String,
    pub label: String,
    pub created_at: i64,
}

impl KeystoreFile {
    pub fn path_for(pubkey: &str) -> Result<PathBuf> {
        let wallets_dir = crate::config::Config::wallets_dir()?;
        Ok(wallets_dir.join(format!("{}.json", pubkey)))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read keystore: {}", path.display()))?;
        let keystore: KeystoreFile = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse keystore: {}", path.display()))?;
        Ok(keystore)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)
            .context("Failed to serialize keystore")?;
        Self::write_secure(path, json.as_bytes())?;
        Ok(())
    }

    fn write_secure(path: &Path, data: &[u8]) -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("Failed to open for write: {}", path.display()))?;
        file.write_all(data)
            .with_context(|| format!("Failed to write: {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(path, perms)?;
        }
        Ok(())
    }

    pub fn list_all() -> Result<Vec<Self>> {
        let wallets_dir = crate::config::Config::wallets_dir()?;
        if !wallets_dir.exists() {
            return Ok(Vec::new());
        }
        let mut stores = Vec::new();
        for entry in fs::read_dir(&wallets_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                match Self::load(&path) {
                    Ok(ks) => stores.push(ks),
                    Err(e) => eprintln!("Warning: could not load {}: {}", path.display(), e),
                }
            }
        }
        stores.sort_by(|a, b| a.label.cmp(&b.label));
        Ok(stores)
    }
}

fn derive_key(passphrase: &str, salt: &[u8], m_cost: u32, t_cost: u32, p_cost: u32) -> [u8; KEY_LEN] {
    let params = argon2::Params::new(m_cost, t_cost, p_cost, Some(KEY_LEN))
        .expect("valid argon2 params");
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .expect("argon2 derivation should not fail");
    key
}

pub fn encrypt_keypair(keypair_bytes: &[u8; 64], passphrase: &str, pubkey: &str, label: &str) -> Result<KeystoreFile> {
    let mut salt = vec![0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    let mut rng = rand::rng();
    rng.fill_bytes(&mut salt);
    rng.fill_bytes(&mut nonce_bytes);

    let mut key = derive_key(passphrase, &salt, ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST);
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::try_from(&key[..]).unwrap());
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, keypair_bytes.as_ref())
        .map_err(|e| FatError::encryption(format!("AES-GCM encrypt failed: {}", e)))?;

    key.zeroize();

    Ok(KeystoreFile {
        version: KEYSTORE_VERSION,
        kdf: "argon2id".to_string(),
        kdf_params: KdfParams {
            salt,
            m_cost: ARGON2_M_COST,
            t_cost: ARGON2_T_COST,
            p_cost: ARGON2_P_COST,
        },
        cipher: "aes-256-gcm".to_string(),
        nonce: nonce_bytes.to_vec(),
        ciphertext,
        pubkey: pubkey.to_string(),
        label: label.to_string(),
        created_at: chrono::Utc::now().timestamp(),
    })
}

pub fn decrypt_keypair(keystore: &KeystoreFile, passphrase: &str) -> Result<[u8; 64]> {
    if keystore.kdf != "argon2id" {
        return Err(FatError::decryption(format!("Unsupported KDF: {}", keystore.kdf)).into());
    }
    if keystore.cipher != "aes-256-gcm" {
        return Err(FatError::decryption(format!("Unsupported cipher: {}", keystore.cipher)).into());
    }

    let kp = &keystore.kdf_params;
    let mut key = derive_key(passphrase, &kp.salt, kp.m_cost, kp.t_cost, kp.p_cost);
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::try_from(&key[..]).unwrap());

    let nonce_bytes: [u8; NONCE_LEN] = keystore
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| FatError::decryption("Invalid nonce length"))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, keystore.ciphertext.as_ref())
        .map_err(|_| FatError::decryption("Wrong passphrase or corrupted keystore"))?;

    key.zeroize();

    if plaintext.len() != 64 {
        let mut pt = plaintext;
        pt.zeroize();
        return Err(FatError::decryption("Decrypted keypair is not 64 bytes").into());
    }

    let mut keypair_bytes = [0u8; 64];
    keypair_bytes.copy_from_slice(&plaintext);

    // Zeroize the plaintext Vec buffer
    let mut pt = plaintext;
    pt.zeroize();

    Ok(keypair_bytes)
}