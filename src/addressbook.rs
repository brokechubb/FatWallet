use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::error::FatError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub label: String,
    pub address: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AddressBook {
    #[serde(default)]
    pub contacts: Vec<Contact>,
}

impl AddressBook {
    pub fn load() -> Result<Self> {
        let path = crate::config::Config::contacts_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read contacts: {}", path.display()))?;
        let book: AddressBook = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse contacts: {}", path.display()))?;
        Ok(book)
    }

    pub fn save(&self) -> Result<()> {
        let path = crate::config::Config::contacts_path()?;
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create dir: {}", dir.display()))?;
        let toml_str = toml::to_string_pretty(self)
            .context("Failed to serialize contacts")?;

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .with_context(|| format!("Failed to open: {}", path.display()))?;
        use std::io::Write;
        file.write_all(toml_str.as_bytes())
            .with_context(|| format!("Failed to write: {}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms)?;
        }
        Ok(())
    }

    pub fn add(&mut self, label: &str, address: &str, note: Option<String>) -> Result<()> {
        if self.contacts.iter().any(|c| c.label.eq_ignore_ascii_case(label)) {
            return Err(FatError::AddressBook(format!("Contact '{}' already exists", label)).into());
        }
        self.contacts.push(Contact {
            label: label.to_string(),
            address: address.to_string(),
            note,
        });
        self.contacts.sort_by(|a, b| a.label.cmp(&b.label));
        self.save()
    }

    pub fn remove(&mut self, label: &str) -> Result<bool> {
        let len_before = self.contacts.len();
        self.contacts.retain(|c| !c.label.eq_ignore_ascii_case(label));
        let removed = self.contacts.len() < len_before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn list(&self) -> &[Contact] {
        &self.contacts
    }

    /// Sync wallet addresses into the address book.
    /// Adds any wallet not already present (matched by address).
    /// Does not overwrite existing contacts.
    pub fn sync_wallets(&mut self, wallets: &[crate::wallet::WalletInfo]) -> Result<()> {
        let mut changed = false;
        for w in wallets {
            let exists = self.contacts.iter().any(|c| c.address == w.pubkey);
            if !exists {
                self.contacts.push(Contact {
                    label: w.label.clone(),
                    address: w.pubkey.clone(),
                    note: Some("auto-synced from wallet".to_string()),
                });
                changed = true;
            }
        }
        if changed {
            self.contacts.sort_by(|a, b| a.label.cmp(&b.label));
            self.save()?;
        }
        Ok(())
    }

    pub fn find(&self, query: &str) -> Option<&Contact> {
        // Try exact label match first, then partial label, then exact address
        self.contacts
            .iter()
            .find(|c| c.label.eq_ignore_ascii_case(query))
            .or_else(|| {
                self.contacts
                    .iter()
                    .find(|c| c.label.to_lowercase().contains(&query.to_lowercase()))
            })
            .or_else(|| {
                self.contacts
                    .iter()
                    .find(|c| c.address == query)
            })
    }
}