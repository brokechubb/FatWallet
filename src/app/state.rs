use std::collections::HashMap;

use crate::addressbook::AddressBook;
use crate::rpc::balance::WalletBalances;
use crate::rpc::transactions::TxHistoryEntry;
use crate::wallet::WalletInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UIMode {
    Dashboard,
    TxDetail,
    Send,
    Receive,
    Import,
    Contacts,
    ContactAdd,
    Swap,
    Help,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Create,
    ImportSeed,
    ImportKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshState {
    Idle,
    Loading,
    Error,
}

pub struct AppState {
    pub wallets: Vec<WalletInfo>,
    pub active_wallet_idx: usize,
    pub balances: Option<WalletBalances>,
    pub transactions: Vec<TxHistoryEntry>,
    pub prices: HashMap<String, f64>,
    pub ui_mode: UIMode,
    pub refresh_state: RefreshState,
    pub last_error: Option<String>,
    pub tx_scroll: usize,
    pub tx_detail_idx: Option<usize>,
    pub help_scroll: u16,
    pub status_message: Option<String>,
    pub should_quit: bool,
    // Input handling for TUI forms
    pub input_field: String,
    pub input_step: usize,
    pub import_mode: ImportMode,
    pub temp_passphrase: String,
    pub temp_seed: String,
    pub temp_label: String,
    pub temp_amount: String,
    pub temp_recipient: String,
    pub temp_token: String,
    pub contacts: Option<AddressBook>,
    pub contact_scroll: usize,
    pub show_contact_picker: bool,
    pub confirm_delete_contact: bool,
}

impl AppState {
    pub fn new(wallets: Vec<WalletInfo>) -> Self {
        Self {
            wallets,
            active_wallet_idx: 0,
            balances: None,
            transactions: Vec::new(),
            prices: HashMap::new(),
            ui_mode: UIMode::Dashboard,
            refresh_state: RefreshState::Idle,
            last_error: None,
            tx_scroll: 0,
            tx_detail_idx: None,
            help_scroll: 0,
            status_message: None,
            should_quit: false,
            input_field: String::new(),
            input_step: 0,
            import_mode: ImportMode::Create,
            temp_passphrase: String::new(),
            temp_seed: String::new(),
            temp_label: String::new(),
            temp_amount: String::new(),
            temp_recipient: String::new(),
            temp_token: String::new(),
            contacts: None,
            contact_scroll: 0,
            show_contact_picker: false,
            confirm_delete_contact: false,
        }
    }

    pub fn active_wallet(&self) -> Option<&WalletInfo> {
        self.wallets.get(self.active_wallet_idx)
    }

    pub fn active_pubkey(&self) -> Option<String> {
        self.active_wallet().map(|w| w.pubkey.clone())
    }

    pub fn switch_wallet(&mut self, idx: usize) {
        if idx < self.wallets.len() {
            self.active_wallet_idx = idx;
            self.balances = None;
            self.transactions.clear();
            self.tx_scroll = 0;
            self.refresh_state = RefreshState::Idle;
        }
    }

    pub fn next_wallet(&mut self) {
        if !self.wallets.is_empty() {
            self.active_wallet_idx = (self.active_wallet_idx + 1) % self.wallets.len();
            self.balances = None;
            self.transactions.clear();
            self.tx_scroll = 0;
            self.refresh_state = RefreshState::Idle;
        }
    }

    pub fn prev_wallet(&mut self) {
        if !self.wallets.is_empty() {
            self.active_wallet_idx = if self.active_wallet_idx == 0 {
                self.wallets.len() - 1
            } else {
                self.active_wallet_idx - 1
            };
            self.balances = None;
            self.transactions.clear();
            self.tx_scroll = 0;
            self.refresh_state = RefreshState::Idle;
        }
    }

    pub fn scroll_tx_down(&mut self) {
        if self.tx_scroll < self.transactions.len().saturating_sub(1) {
            self.tx_scroll += 1;
        }
    }

    pub fn scroll_tx_up(&mut self) {
        if self.tx_scroll > 0 {
            self.tx_scroll -= 1;
        }
    }

    pub fn set_balances(&mut self, balances: WalletBalances) {
        self.balances = Some(balances);
        self.refresh_state = RefreshState::Idle;
        self.last_error = None;
    }

    pub fn set_transactions(&mut self, txs: Vec<TxHistoryEntry>) {
        self.transactions = txs;
    }

    pub fn set_error(&mut self, msg: String) {
        self.refresh_state = RefreshState::Error;
        self.last_error = Some(msg);
    }
}