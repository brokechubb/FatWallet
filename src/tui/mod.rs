pub mod dashboard;

use std::time::Duration;

use std::io::Write;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app::message::Message;
use crate::app::state::{AppState, RefreshState, UIMode};
use crate::config::Config;
use crate::price::PriceService;
use crate::rpc::{balance, transactions, Rpc};
use crate::wallet;

pub fn run() -> Result<()> {
    let wallets = wallet::list_wallets()?;
    if wallets.is_empty() {
        eprintln!("No wallets found. Use 'fatwallet create' or 'fatwallet import' first.");
        return Ok(());
    }

    // Sync wallet addresses into address book
    if let Ok(mut book) = crate::addressbook::AddressBook::load() {
        let _ = book.sync_wallets(&wallets);
    }

    let config = Config::load()?;

    // Setup terminal manually for precise cleanup
    enable_raw_mode()?;
    execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::SetTitle("FatWallet"),
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async {
        run_event_loop(&mut terminal, wallets, config).await
    });

    // Restore terminal synchronously before dropping runtime
    disable_raw_mode()?;
    execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::terminal::SetTitle(""),
    )?;
    std::io::stdout().flush()?;

    drop(rt);

    result
}

async fn run_event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    wallets: Vec<wallet::WalletInfo>,
    config: Config,
) -> Result<()> {
    let mut state = AppState::new(wallets);
    let (event_tx, mut event_rx) = mpsc::channel::<Message>(64);

    // Cancellation token to stop the poller task cleanly
    let cancel = CancellationToken::new();

    // crossterm event poller — uses blocking poll in a spawn_blocking task
    // to avoid the 50ms sleep lag
    let (tx_key, mut rx_key) = mpsc::channel::<crossterm::event::KeyEvent>(32);
    let (tx_resize, mut rx_resize) = mpsc::channel::<(u16, u16)>(8);
    let cancel_clone = cancel.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            if cancel_clone.is_cancelled() {
                break;
            }
            // Block until an event is available (no busy-polling)
            if let Ok(true) = crossterm::event::poll(Duration::from_millis(50)) {
                match crossterm::event::read() {
                    Ok(Event::Key(key)) => {
                        if key.kind == KeyEventKind::Press {
                            // Use blocking_send to avoid async in spawn_blocking
                            if tx_key.blocking_send(key).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Event::Resize(w, h)) => {
                        let _ = tx_resize.blocking_send((w, h));
                    }
                    _ => {}
                }
            }
        }
    });

    let rpc = Rpc::new(&config.rpc_url);
    let price_svc = PriceService::new(&config.jupiter_api_url, {
        if config.jupiter_api_key.is_empty() { None } else { Some(config.jupiter_api_key.clone()) }
    });

    // Initial refresh
    state.refresh_state = RefreshState::Loading;
    spawn_balance_refresh(&rpc, &price_svc, &state, event_tx.clone());

    let mut refresh_interval = tokio::time::interval(Duration::from_secs(
        config.refresh_interval_secs.max(10),
    ));
    refresh_interval.tick().await;

    loop {
        terminal.draw(|f| dashboard::render(f, &state))?;

        tokio::select! {
            Some(key) = rx_key.recv() => {
                handle_key(key, &mut state, &rpc, &price_svc, &event_tx, terminal);
                if state.should_quit {
                    cancel.cancel();
                    break;
                }
            }
            Some((w, h)) = rx_resize.recv() => {
                terminal.resize(ratatui::layout::Rect::new(0, 0, w, h))?;
            }
            Some(msg) = event_rx.recv() => {
                match msg {
                    Message::BalancesLoaded(balances) => {
                        state.set_balances(balances);
                        spawn_tx_refresh(&rpc, &state, event_tx.clone());
                    }
                    Message::TransactionsLoaded(txs) => {
                        state.set_transactions(txs);
                    }
                    Message::SendResult(Ok(sig)) => {
                        state.status_message = Some(format!("Sent! https://solscan.io/tx/{}", sig));
                        state.refresh_state = RefreshState::Loading;
                        spawn_balance_refresh(&rpc, &price_svc, &state, event_tx.clone());
                    }
                    Message::SendResult(Err(e)) => {
                        state.status_message = Some(format!("Send failed: {}", e));
                    }
                    Message::SwapResult(Ok(sig)) => {
                        state.status_message = Some(format!("Swap complete! https://solscan.io/tx/{}", sig));
                        state.refresh_state = RefreshState::Loading;
                        spawn_balance_refresh(&rpc, &price_svc, &state, event_tx.clone());
                    }
                    Message::SwapResult(Err(e)) => {
                        state.status_message = Some(format!("Swap failed: {}", e));
                    }
                    Message::Error(e) => {
                        state.set_error(e);
                    }
                    _ => {}
                }
            }
            _ = refresh_interval.tick() => {
                if state.refresh_state != RefreshState::Loading && state.ui_mode == UIMode::Dashboard {
                    state.refresh_state = RefreshState::Loading;
                    spawn_balance_refresh(&rpc, &price_svc, &state, event_tx.clone());
                }
            }
        }
    }

    Ok(())
}

fn handle_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    rpc: &Rpc,
    price_svc: &PriceService,
    event_tx: &mpsc::Sender<Message>,
    _terminal: &Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) {
    match state.ui_mode {
        UIMode::Dashboard | UIMode::TxDetail | UIMode::Help => handle_dashboard_key(key, state, rpc, price_svc, event_tx),
        UIMode::Import => handle_import_key(key, state, rpc, price_svc, event_tx),
        UIMode::Send => handle_send_key(key, state, event_tx),
        UIMode::Swap => handle_swap_key(key, state, event_tx),
        UIMode::Receive => handle_receive_key(key, state),
        UIMode::Contacts => handle_contacts_key(key, state),
        UIMode::ContactAdd => handle_contact_add_key(key, state),
        _ => {}
    }
}

fn handle_dashboard_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    rpc: &Rpc,
    price_svc: &PriceService,
    event_tx: &mpsc::Sender<Message>,
) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => { state.should_quit = true; }
        KeyCode::Tab => {
            state.next_wallet();
            state.ui_mode = UIMode::Dashboard;
            state.tx_detail_idx = None;
            trigger_refresh(state, rpc, price_svc, event_tx);
        }
        KeyCode::BackTab => {
            state.prev_wallet();
            state.ui_mode = UIMode::Dashboard;
            state.tx_detail_idx = None;
            trigger_refresh(state, rpc, price_svc, event_tx);
        }
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as u8 - b'1') as usize;
            if idx < state.wallets.len() {
                state.switch_wallet(idx);
                trigger_refresh(state, rpc, price_svc, event_tx);
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT) || key.code == KeyCode::Char('R') {
                state.ui_mode = UIMode::Receive;
            } else {
                trigger_refresh(state, rpc, price_svc, event_tx);
            }
        }
        KeyCode::Char('n') => {
            state.ui_mode = UIMode::Import;
            state.import_mode = crate::app::state::ImportMode::Create;
            state.input_field.clear();
            state.input_step = 0;
            state.status_message = None;
        }
        KeyCode::Char('i') => {
            state.ui_mode = UIMode::Import;
            state.import_mode = crate::app::state::ImportMode::ImportSeed;
            state.input_field.clear();
            state.input_step = 0;
            state.status_message = None;
        }
        KeyCode::Char('k') => {
            state.ui_mode = UIMode::Import;
            state.import_mode = crate::app::state::ImportMode::ImportKey;
            state.input_field.clear();
            state.input_step = 0;
            state.status_message = None;
        }
        KeyCode::Char('s') => {
            state.ui_mode = UIMode::Send;
            state.input_field.clear();
            state.input_step = 0;
            state.status_message = None;
            state.contacts = crate::addressbook::AddressBook::load().ok();
        }
        KeyCode::Char('a') => { state.ui_mode = UIMode::Contacts; state.input_field.clear(); state.input_step = 0; state.contacts = crate::addressbook::AddressBook::load().ok(); state.status_message = None; }
        KeyCode::Char('h') => { state.ui_mode = UIMode::Help; state.help_scroll = 0; }
        KeyCode::Char('x') => {
            state.ui_mode = UIMode::Swap;
            state.input_field.clear();
            state.input_step = 0;
            state.status_message = None;
        }
        KeyCode::Down => {
            match state.ui_mode {
                UIMode::Dashboard => state.scroll_tx_down(),
                UIMode::Help => { state.help_scroll = state.help_scroll.saturating_add(1); }
                _ => {}
            }
        }
        KeyCode::Up => {
            match state.ui_mode {
                UIMode::Dashboard => state.scroll_tx_up(),
                UIMode::Help => { state.help_scroll = state.help_scroll.saturating_sub(1); }
                _ => {}
            }
        }
        KeyCode::Enter => {
            match state.ui_mode {
                UIMode::Dashboard => {
                    if !state.transactions.is_empty() {
                        state.tx_detail_idx = Some(state.tx_scroll);
                        state.ui_mode = UIMode::TxDetail;
                    }
                }
                UIMode::TxDetail => {
                    state.ui_mode = UIMode::Dashboard;
                    state.tx_detail_idx = None;
                }
                _ => {}
            }
        }
        KeyCode::Esc => {
            match state.ui_mode {
                UIMode::TxDetail => { state.ui_mode = UIMode::Dashboard; state.tx_detail_idx = None; }
                UIMode::Help => { state.ui_mode = UIMode::Dashboard; state.help_scroll = 0; }
                _ => {}
            }
        }
        KeyCode::Char('c') => {
            if state.ui_mode == UIMode::TxDetail {
                if let Some(idx) = state.tx_detail_idx {
                    if let Some(tx) = state.transactions.get(idx) {
                        let sig = tx.signature.clone();
                        match arboard::Clipboard::new().and_then(|mut c| c.set_text(sig)) {
                            Ok(_) => state.status_message = Some("Signature copied to clipboard".to_string()),
                            Err(_) => state.status_message = Some("Failed to copy".to_string()),
                        }
                    }
                }
            }
            if state.ui_mode == UIMode::Receive {
                if let Some(addr) = state.active_pubkey() {
                    match arboard::Clipboard::new().and_then(|mut c| c.set_text(addr)) {
                        Ok(_) => state.status_message = Some("Address copied to clipboard".to_string()),
                        Err(_) => state.status_message = Some("Failed to copy".to_string()),
                    }
                }
            }
        }
        KeyCode::Char('o') => {
            if state.ui_mode == UIMode::TxDetail {
                if let Some(idx) = state.tx_detail_idx {
                    if let Some(tx) = state.transactions.get(idx) {
                        let url = format!("https://solscan.io/tx/{}", tx.signature);
                        match open::that(&url) {
                            Ok(_) => state.status_message = Some("Opened in browser".to_string()),
                            Err(_) => state.status_message = Some("Failed to open browser".to_string()),
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn handle_import_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    _rpc: &Rpc,
    _price_svc: &PriceService,
    _event_tx: &mpsc::Sender<Message>,
) {
    match key.code {
        KeyCode::Esc => {
            state.ui_mode = UIMode::Dashboard;
            state.input_field.clear();
            state.input_step = 0;
        }
        KeyCode::Enter => {
            match state.import_mode {
                crate::app::state::ImportMode::Create => {
                    match state.input_step {
                        0 => {
                            // Enter passphrase (will need confirmation)
                            state.temp_passphrase = state.input_field.clone();
                            state.input_field.clear();
                            state.input_step = 1;
                        }
                        1 => {
                            // Confirm passphrase
                            if state.input_field != state.temp_passphrase {
                                state.status_message = Some("Passphrases do not match".to_string());
                                state.input_field.clear();
                                state.input_step = 0;
                                state.temp_passphrase.clear();
                                return;
                            }
                            state.input_field.clear();
                            state.input_step = 2; // now enter label
                        }
                        2 => {
                            // Label entered — create the wallet
                            let label = state.input_field.clone();
                            let passphrase = state.temp_passphrase.clone();
                            match wallet::create_wallet(&label, &passphrase) {
                                Ok((info, mnemonic)) => {
                                    state.status_message = Some(format!(
                                        "Wallet created: {} ({}) — seed: {}",
                                        info.label, info.pubkey, mnemonic
                                    ));
                                    // Reload wallet list
                                    if let Ok(wallets) = wallet::list_wallets() {
                                        state.wallets = wallets.clone();
                                        if let Some(idx) = state.wallets.iter().position(|w| w.pubkey == info.pubkey) {
                                            state.active_wallet_idx = idx;
                                        }
                                        if let Ok(mut book) = crate::addressbook::AddressBook::load() {
                                            let _ = book.sync_wallets(&wallets);
                                        }
                                    }
                                    state.ui_mode = UIMode::Dashboard;
                                    state.input_field.clear();
                                    state.input_step = 0;
                                    state.temp_passphrase.clear();
                                }
                                Err(e) => {
                                    state.status_message = Some(format!("Error: {}", e));
                                    state.input_field.clear();
                                    state.input_step = 0;
                                    state.temp_passphrase.clear();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                crate::app::state::ImportMode::ImportSeed => {
                    match state.input_step {
                        0 => {
                            state.temp_passphrase = state.input_field.clone();
                            state.input_field.clear();
                            state.input_step = 1;
                        }
                        1 => {
                            if state.input_field != state.temp_passphrase {
                                state.status_message = Some("Passphrases do not match".to_string());
                                state.input_field.clear();
                                state.input_step = 0;
                                state.temp_passphrase.clear();
                                return;
                            }
                            state.input_field.clear();
                            state.input_step = 2;
                        }
                        2 => {
                            // Seed phrase entered
                            state.temp_seed = state.input_field.clone();
                            state.input_field.clear();
                            state.input_step = 3;
                        }
                        3 => {
                            // Label entered — import the wallet
                            let label = state.input_field.clone();
                            let passphrase = state.temp_passphrase.clone();
                            let seed = state.temp_seed.clone();
                            match wallet::import_from_seed_phrase(&seed, &passphrase, &label, 0, 0) {
                                Ok(info) => {
                                    state.status_message = Some(format!(
                                        "Wallet imported: {} ({})", info.label, info.pubkey
                                    ));
                                    if let Ok(wallets) = wallet::list_wallets() {
                                        state.wallets = wallets.clone();
                                        if let Some(idx) = state.wallets.iter().position(|w| w.pubkey == info.pubkey) {
                                            state.active_wallet_idx = idx;
                                        }
                                        if let Ok(mut book) = crate::addressbook::AddressBook::load() {
                                            let _ = book.sync_wallets(&wallets);
                                        }
                                    }
                                    state.ui_mode = UIMode::Dashboard;
                                    state.input_field.clear();
                                    state.input_step = 0;
                                    state.temp_passphrase.clear();
                                    state.temp_seed.clear();
                                }
                                Err(e) => {
                                    state.status_message = Some(format!("Error: {}", e));
                                    state.input_field.clear();
                                    state.input_step = 2;
                                    state.temp_seed.clear();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                crate::app::state::ImportMode::ImportKey => {
                    match state.input_step {
                        0 => {
                            state.temp_passphrase = state.input_field.clone();
                            state.input_field.clear();
                            state.input_step = 1;
                        }
                        1 => {
                            if state.input_field != state.temp_passphrase {
                                state.status_message = Some("Passphrases do not match".to_string());
                                state.input_field.clear();
                                state.input_step = 0;
                                state.temp_passphrase.clear();
                                return;
                            }
                            state.input_field.clear();
                            state.input_step = 2;
                        }
                        2 => {
                            // Private key entered
                            state.temp_seed = state.input_field.clone();
                            state.input_field.clear();
                            state.input_step = 3;
                        }
                        3 => {
                            // Label entered
                            let label = state.input_field.clone();
                            let passphrase = state.temp_passphrase.clone();
                            let key = state.temp_seed.clone();
                            match wallet::import_from_private_key(&key, &passphrase, &label) {
                                Ok(info) => {
                                    state.status_message = Some(format!(
                                        "Wallet imported: {} ({})", info.label, info.pubkey
                                    ));
                                    if let Ok(wallets) = wallet::list_wallets() {
                                        state.wallets = wallets.clone();
                                        if let Some(idx) = state.wallets.iter().position(|w| w.pubkey == info.pubkey) {
                                            state.active_wallet_idx = idx;
                                        }
                                        if let Ok(mut book) = crate::addressbook::AddressBook::load() {
                                            let _ = book.sync_wallets(&wallets);
                                        }
                                    }
                                    state.ui_mode = UIMode::Dashboard;
                                    state.input_field.clear();
                                    state.input_step = 0;
                                    state.temp_passphrase.clear();
                                    state.temp_seed.clear();
                                }
                                Err(e) => {
                                    state.status_message = Some(format!("Error: {}", e));
                                    state.input_field.clear();
                                    state.input_step = 2;
                                    state.temp_seed.clear();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        KeyCode::Backspace => { state.input_field.pop(); }
        KeyCode::Char(c) => { state.input_field.push(c); }
        _ => {}
    }
}

fn handle_send_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    event_tx: &mpsc::Sender<Message>,
) {
    // At recipient step, allow contact picker navigation
    if state.input_step == 2 {
        match key.code {
            KeyCode::Up => {
                if state.contact_scroll > 0 {
                    state.contact_scroll -= 1;
                }
                return;
            }
            KeyCode::Down => {
                let count = state.contacts.as_ref().map(|b| b.list().len()).unwrap_or(0);
                if state.contact_scroll < count.saturating_sub(1) {
                    state.contact_scroll += 1;
                }
                return;
            }
            KeyCode::Tab => {
                state.show_contact_picker = !state.show_contact_picker;
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc => {
            state.ui_mode = UIMode::Dashboard;
            state.input_field.clear();
            state.input_step = 0;
            state.temp_amount.clear();
            state.temp_recipient.clear();
            state.temp_token.clear();
            state.temp_passphrase.clear();
            state.contact_scroll = 0;
            state.show_contact_picker = false;
        }
        KeyCode::Enter => {
            match state.input_step {
                0 => {
                    state.temp_token = state.input_field.clone();
                    state.input_field.clear();
                    state.input_step = 1;
                }
                1 => {
                    let input = state.input_field.trim().to_string();
                    let token = state.temp_token.clone();
                    match parse_amount_input(state, &input, &token) {
                        Some(amount_str) => {
                            state.temp_amount = amount_str;
                            state.input_field.clear();
                            state.contact_scroll = 0;
                            if !state.temp_recipient.is_empty() {
                                // Recipient pre-filled from address book — skip to passphrase
                                state.input_step = 3;
                            } else {
                                state.input_step = 2;
                            }
                        }
                        None => {}
                    }
                }
                2 => {
                    // If contact picker is active, use selected contact
                    let recipient = if state.show_contact_picker {
                        state.contacts.as_ref()
                            .and_then(|b| b.list().get(state.contact_scroll))
                            .map(|c| c.address.clone())
                            .unwrap_or_default()
                    } else if state.input_field.is_empty() && !state.temp_recipient.is_empty() {
                        // Pre-filled from address book
                        state.temp_recipient.clone()
                    } else {
                        let input = state.input_field.clone();
                        let resolved = state.contacts.as_ref()
                            .and_then(|book| book.find(&input))
                            .map(|c| c.address.clone());
                        resolved.unwrap_or(input)
                    };
                    if recipient.is_empty() {
                        state.status_message = Some("No recipient selected".to_string());
                        return;
                    }
                    state.temp_recipient = recipient;
                    state.input_field.clear();
                    state.input_step = 3;
                    state.show_contact_picker = false;
                }
                3 => {
                    state.temp_passphrase = state.input_field.clone();
                    state.input_field.clear();
                    state.input_step = 4;
                }
                4 => {
                    let pubkey = state.active_pubkey().unwrap_or_default();
                    let passphrase = state.temp_passphrase.clone();
                    let token = state.temp_token.clone();
                    let amount_str = state.temp_amount.clone();
                    let recipient = state.temp_recipient.clone();

                    match wallet::unlock_wallet(&pubkey, &passphrase) {
                        Ok(unlocked) => {
                            let amount: f64 = amount_str.parse().unwrap_or(0.0);
                            if amount <= 0.0 {
                                state.status_message = Some("Invalid amount".to_string());
                                state.input_field.clear();
                                state.input_step = 1;
                                state.temp_passphrase.clear();
                                return;
                            }

                            let (mint, decimals) = resolve_token_for_send(&token);

                            let event_tx2 = event_tx.clone();
                            let keypair_bytes = unlocked.keypair.to_bytes();
                            tokio::spawn(async move {
                                let rpc = match crate::rpc::Rpc::from_config() {
                                    Ok(r) => r,
                                    Err(e) => {
                                        let _ = event_tx2.send(Message::SendResult(Err(e.to_string()))).await;
                                        return;
                                    }
                                };
                                let kp = match solana_keypair::Keypair::try_from(&keypair_bytes[..]) {
                                    Ok(k) => k,
                                    Err(e) => {
                                        let _ = event_tx2.send(Message::SendResult(Err(e.to_string()))).await;
                                        return;
                                    }
                                };
                                match crate::rpc::transfer::transfer(&rpc, &kp, &recipient, &mint, amount, decimals).await {
                                    Ok(sig) => {
                                        let _ = event_tx2.send(Message::SendResult(Ok(sig))).await;
                                    }
                                    Err(e) => {
                                        let _ = event_tx2.send(Message::SendResult(Err(e.to_string()))).await;
                                    }
                                }
                            });

                            state.status_message = Some("Sending...".to_string());
                            state.ui_mode = UIMode::Dashboard;
                            state.input_field.clear();
                            state.input_step = 0;
                            state.temp_passphrase.clear();
                        }
                        Err(e) => {
                            state.status_message = Some(format!("Unlock failed: {}", e));
                            state.input_field.clear();
                            state.input_step = 3;
                            state.temp_passphrase.clear();
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Backspace => {
            if state.input_step != 2 || !state.show_contact_picker {
                state.input_field.pop();
            }
        }
        KeyCode::Char(c) => {
            if state.input_step != 2 || !state.show_contact_picker {
                state.input_field.push(c);
            }
        }
        _ => {}
    }
}

fn resolve_token_for_send(token: &str) -> (String, u8) {
    match token.to_uppercase().as_str() {
        "SOL" => ("So11111111111111111111111111111111111111112".to_string(), 9),
        "USDC" => ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), 6),
        "USDT" => ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), 6),
        _ => (token.to_string(), 9),
    }
}

/// Resolve "all"/"max" to the actual balance for the given token.
/// For SOL, reserves 0.01 SOL for gas fees.
/// Returns None if balance is unavailable or token not found.
fn resolve_max_amount(state: &AppState, token: &str) -> Option<f64> {
    let token_upper = token.to_uppercase();
    if token_upper == "SOL" {
        let bal = state.balances.as_ref()?.sol_balance;
        let max = bal - 0.01; // reserve for gas
        if max > 0.0 { Some(max) } else { None }
    } else {
        let bal = state.balances.as_ref()?.tokens.iter()
            .find(|t| t.symbol.eq_ignore_ascii_case(&token_upper) || t.mint == token)?;
        if bal.amount > 0.0 { Some(bal.amount) } else { None }
    }
}

/// Parse the amount input field, handling "all"/"max" keywords, $USD conversion, and raw amounts.
/// Returns the resolved amount string, or None if resolution failed (with status set).
fn parse_amount_input(state: &mut AppState, input: &str, token: &str) -> Option<String> {
    let trimmed = input.trim();
    let lower = trimmed.to_lowercase();
    if lower == "all" || lower == "max" {
        match resolve_max_amount(state, token) {
            Some(amt) => {
                state.status_message = Some(format!("Max: {:.9} {}", amt, token));
                Some(format!("{:.9}", amt))
            }
            None => {
                state.status_message = Some(format!("No balance available for {}", token));
                None
            }
        }
    } else if trimmed.starts_with('$') {
        let usd: f64 = trimmed[1..].parse().unwrap_or(0.0);
        if usd <= 0.0 {
            state.status_message = Some("Invalid USD amount".to_string());
            return None;
        }
        let token_upper = token.to_uppercase();
        let price = if token_upper == "SOL" {
            state.balances.as_ref().and_then(|b| b.sol_usd_price)
        } else {
            state.balances.as_ref().and_then(|b| {
                b.tokens.iter()
                    .find(|t| t.symbol.eq_ignore_ascii_case(&token_upper) || t.mint == token)
                    .and_then(|t| t.usd_price)
            })
        };
        match price {
            Some(p) if p > 0.0 => {
                let token_amount = usd / p;
                state.status_message = Some(format!("${:.2} = {:.9} {}", usd, token_amount, token));
                Some(format!("{:.9}", token_amount))
            }
            _ => {
                state.status_message = Some(format!("No price available for {} — enter amount in token units", token));
                None
            }
        }
    } else {
        Some(trimmed.to_string())
    }
}

fn handle_swap_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    event_tx: &mpsc::Sender<Message>,
) {
    match key.code {
        KeyCode::Esc => {
            state.ui_mode = UIMode::Dashboard;
            state.input_field.clear();
            state.input_step = 0;
            state.temp_token.clear();
            state.temp_amount.clear();
            state.temp_passphrase.clear();
            state.temp_label.clear();
        }
        KeyCode::Enter => {
            match state.input_step {
                0 => {
                    // Token in
                    state.temp_token = state.input_field.clone();
                    state.input_field.clear();
                    state.input_step = 1;
                }
                1 => {
                    // Token out
                    state.temp_label = state.input_field.clone();
                    state.input_field.clear();
                    state.input_step = 2;
                }
                2 => {
                    // Amount — support all/max, $USD conversion, or raw amount
                    let input = state.input_field.trim().to_string();
                    let token = state.temp_token.clone();
                    match parse_amount_input(state, &input, &token) {
                        Some(amount_str) => {
                            state.temp_amount = amount_str;
                            state.input_field.clear();
                            state.input_step = 3;
                        }
                        None => {}
                    }
                }
                3 => {
                    // Passphrase
                    state.temp_passphrase = state.input_field.clone();
                    state.input_field.clear();
                    state.input_step = 4;
                }
                4 => {
                    // Confirm — execute swap
                    let pubkey = state.active_pubkey().unwrap_or_default();
                    let passphrase = state.temp_passphrase.clone();
                    let input_token = state.temp_token.clone();
                    let output_token = state.temp_label.clone();
                    let amount_str = state.temp_amount.clone();

                    match wallet::unlock_wallet(&pubkey, &passphrase) {
                        Ok(unlocked) => {
                            let amount: f64 = amount_str.parse().unwrap_or(0.0);
                            if amount <= 0.0 {
                                state.status_message = Some("Invalid amount".to_string());
                                state.input_field.clear();
                                state.input_step = 2;
                                state.temp_passphrase.clear();
                                return;
                            }

                            let (input_mint, _) = resolve_token_for_send(&input_token);
                            let (output_mint, _) = resolve_token_for_send(&output_token);

                            let event_tx2 = event_tx.clone();
                            let keypair_bytes = unlocked.keypair.to_bytes();
                            tokio::spawn(async move {
                                let rpc = match crate::rpc::Rpc::from_config() {
                                    Ok(r) => r,
                                    Err(e) => {
                                        let _ = event_tx2.send(Message::SwapResult(Err(e.to_string()))).await;
                                        return;
                                    }
                                };
                                let kp = match solana_keypair::Keypair::try_from(&keypair_bytes[..]) {
                                    Ok(k) => k,
                                    Err(e) => {
                                        let _ = event_tx2.send(Message::SwapResult(Err(e.to_string()))).await;
                                        return;
                                    }
                                };
                                match crate::rpc::swap::gasless_swap(&rpc, &kp, &input_mint, &output_mint, amount).await {
                                    Ok(sig) => {
                                        let _ = event_tx2.send(Message::SwapResult(Ok(sig))).await;
                                    }
                                    Err(e) => {
                                        let _ = event_tx2.send(Message::SwapResult(Err(e.to_string()))).await;
                                    }
                                }
                            });

                            state.status_message = Some("Swapping...".to_string());
                            state.ui_mode = UIMode::Dashboard;
                            state.input_field.clear();
                            state.input_step = 0;
                            state.temp_passphrase.clear();
                        }
                        Err(e) => {
                            state.status_message = Some(format!("Unlock failed: {}", e));
                            state.input_field.clear();
                            state.input_step = 3;
                            state.temp_passphrase.clear();
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Backspace => { state.input_field.pop(); }
        KeyCode::Char(c) => { state.input_field.push(c); }
        _ => {}
    }
}

fn handle_receive_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
) {
    match key.code {
        KeyCode::Esc => { state.ui_mode = UIMode::Dashboard; }
        KeyCode::Char('c') => {
            if let Some(addr) = state.active_pubkey() {
                match arboard::Clipboard::new().and_then(|mut c| c.set_text(addr)) {
                    Ok(_) => state.status_message = Some("Address copied to clipboard".to_string()),
                    Err(_) => state.status_message = Some("Failed to copy".to_string()),
                }
            }
        }
        _ => {}
    }
}

fn handle_contacts_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
) {
    if state.confirm_delete_contact {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                state.confirm_delete_contact = false;
                if let Some(ref mut book) = state.contacts {
                    let contacts = book.list().to_vec();
                    if state.contact_scroll < contacts.len() {
                        let label = contacts[state.contact_scroll].label.clone();
                        match book.remove(&label) {
                            Ok(_) => state.status_message = Some(format!("Contact '{}' removed", label)),
                            Err(e) => state.status_message = Some(format!("Error: {}", e)),
                        }
                        let count = book.list().len();
                        if state.contact_scroll >= count && count > 0 {
                            state.contact_scroll = count - 1;
                        }
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                state.confirm_delete_contact = false;
                state.status_message = Some("Delete cancelled".to_string());
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            state.ui_mode = UIMode::Dashboard;
            state.input_field.clear();
            state.input_step = 0;
            state.contact_scroll = 0;
        }
        KeyCode::Up => {
            if state.contact_scroll > 0 {
                state.contact_scroll -= 1;
            }
        }
        KeyCode::Down => {
            let count = state.contacts.as_ref().map(|b| b.list().len()).unwrap_or(0);
            if state.contact_scroll < count.saturating_sub(1) {
                state.contact_scroll += 1;
            }
        }
        KeyCode::Char('a') => {
            state.ui_mode = UIMode::ContactAdd;
            state.input_field.clear();
            state.input_step = 0;
            state.temp_label.clear();
            state.status_message = None;
        }
        KeyCode::Char('d') => {
            if let Some(ref book) = state.contacts {
                let contacts = book.list().to_vec();
                if state.contact_scroll < contacts.len() {
                    state.confirm_delete_contact = true;
                    state.status_message = Some(format!(
                        "Delete '{}'? Press [y] to confirm, [n] to cancel",
                        contacts[state.contact_scroll].label
                    ));
                }
            }
        }
        KeyCode::Char('s') => {
            // Send to selected contact
            if let Some(ref book) = state.contacts {
                let contacts = book.list().to_vec();
                if state.contact_scroll < contacts.len() {
                    state.temp_recipient = contacts[state.contact_scroll].address.clone();
                    state.ui_mode = UIMode::Send;
                    state.input_field.clear();
                    state.input_step = 0;
                    state.temp_token.clear();
                    state.temp_amount.clear();
                    state.temp_passphrase.clear();
                    state.status_message = Some(format!("Sending to: {}", contacts[state.contact_scroll].label));
                }
            }
        }
        KeyCode::Char('c') => {
            // Copy selected contact address to clipboard
            if let Some(ref book) = state.contacts {
                let contacts = book.list().to_vec();
                if state.contact_scroll < contacts.len() {
                    let addr = contacts[state.contact_scroll].address.clone();
                    match arboard::Clipboard::new().and_then(|mut c| c.set_text(addr)) {
                        Ok(_) => state.status_message = Some("Address copied to clipboard".to_string()),
                        Err(_) => state.status_message = Some("Failed to copy".to_string()),
                    }
                }
            }
        }
        KeyCode::Enter => {
            if let Some(ref book) = state.contacts {
                let contacts = book.list().to_vec();
                if state.contact_scroll < contacts.len() {
                    state.temp_recipient = contacts[state.contact_scroll].address.clone();
                    state.ui_mode = UIMode::Send;
                    state.input_field.clear();
                    state.input_step = 0;
                    state.temp_token.clear();
                    state.temp_amount.clear();
                    state.temp_passphrase.clear();
                    state.status_message = Some(format!("Sending to: {}", contacts[state.contact_scroll].label));
                }
            }
        }
        _ => {}
    }
}

fn handle_contact_add_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
) {
    match key.code {
        KeyCode::Esc => {
            state.ui_mode = UIMode::Contacts;
            state.input_field.clear();
            state.input_step = 0;
            state.temp_label.clear();
        }
        KeyCode::Enter => {
            match state.input_step {
                0 => {
                    state.temp_label = state.input_field.clone();
                    state.input_field.clear();
                    state.input_step = 1;
                }
                1 => {
                    let label = state.temp_label.clone();
                    let address = state.input_field.clone();
                    if let Some(ref mut book) = state.contacts {
                        match book.add(&label, &address, None) {
                            Ok(_) => state.status_message = Some(format!("Contact '{}' added", label)),
                            Err(e) => state.status_message = Some(format!("Error: {}", e)),
                        }
                    }
                    state.input_field.clear();
                    state.input_step = 0;
                    state.temp_label.clear();
                    state.ui_mode = UIMode::Contacts;
                }
                _ => {}
            }
        }
        KeyCode::Backspace => { state.input_field.pop(); }
        KeyCode::Char(c) => { state.input_field.push(c); }
        _ => {}
    }
}

fn spawn_balance_refresh(
    rpc: &Rpc,
    price_svc: &PriceService,
    state: &AppState,
    tx: mpsc::Sender<Message>,
) {
    if let Some(pubkey) = state.active_pubkey() {
        let rpc = Rpc::new(&rpc.url);
        let price_svc = PriceService::new(&price_svc.api_url, price_svc.api_key.clone());
        tokio::spawn(async move {
            // Small delay to allow RPC to reflect recent transactions
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            match balance::fetch_balances(&rpc, &pubkey).await {
                Ok(mut balances) => {
                    let mints: Vec<String> = balances.tokens.iter().map(|t| t.mint.clone()).collect();

                    // Fetch token metadata for unknown mints
                    let cache = crate::token_metadata::fetch_and_cache_metadata(&mints).await.unwrap_or_default();
                    for token in &mut balances.tokens {
                        if let Some(sym) = cache.get_symbol(&token.mint) {
                            token.symbol = sym;
                        }
                    }

                    let prices = price_svc.get_prices(&mints).await.unwrap_or_default();
                    balance::enrich_with_prices(&mut balances, &prices);
                    let _ = tx.send(Message::BalancesLoaded(balances)).await;
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(e.to_string())).await;
                }
            }
        });
    }
}

fn spawn_tx_refresh(rpc: &Rpc, state: &AppState, tx: mpsc::Sender<Message>) {
    if let Some(pubkey) = state.active_pubkey() {
        let rpc = Rpc::new(&rpc.url);
        tokio::spawn(async move {
            match transactions::fetch_transactions(&rpc, &pubkey, 20).await {
                Ok(mut txs) => {
                    let mints: Vec<String> = txs.iter().filter_map(|t| t.token_mint.clone()).collect();
                    if !mints.is_empty() {
                        if let Ok(cache) = crate::token_metadata::fetch_and_cache_metadata(&mints).await {
                            for entry in &mut txs {
                                if let Some(ref mint) = entry.token_mint {
                                    if let Some(sym) = cache.get_symbol(mint) {
                                        entry.token_symbol = sym;
                                    }
                                }
                            }
                        }
                    }
                    let _ = tx.send(Message::TransactionsLoaded(txs)).await;
                }
                Err(e) => { let _ = tx.send(Message::Error(e.to_string())).await; }
            }
        });
    }
}

fn trigger_refresh(
    state: &mut AppState,
    rpc: &Rpc,
    price_svc: &PriceService,
    event_tx: &mpsc::Sender<Message>,
) {
    state.refresh_state = RefreshState::Loading;
    state.status_message = None;
    spawn_balance_refresh(rpc, price_svc, state, event_tx.clone());
}