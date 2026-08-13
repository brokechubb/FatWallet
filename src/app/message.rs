use crate::rpc::balance::WalletBalances;
use crate::rpc::transactions::TxHistoryEntry;

/// Messages flowing through the async event loop.
#[derive(Debug)]
pub enum Message {
    /// Keyboard / terminal event
    Key(crossterm::event::KeyEvent),
    /// Balance refresh completed (wallet pubkey, balances)
    BalancesLoaded(String, WalletBalances),
    /// Transaction history loaded
    TransactionsLoaded(Vec<TxHistoryEntry>),
    /// Send transaction result
    SendResult(Result<String, String>),
    /// Send transaction progress update
    SendProgress(String),
    /// Swap transaction result
    SwapResult(Result<String, String>),
    /// Swap transaction progress update
    SwapProgress(String),
    /// Non-active wallet total loaded
    WalletTotalLoaded(String, f64),
    /// Refresh tick (periodic)
    RefreshTick,
    /// Error during async operation
    Error(String),
    /// Quit the app
    Quit,
}