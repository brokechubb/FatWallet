use fatwallet::addressbook;
use fatwallet::config;
use fatwallet::keyring_helper;
use fatwallet::price;
use fatwallet::rpc;
use fatwallet::tui;
use fatwallet::wallet;

use std::io::{self, BufRead, Write};

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fatwallet", version, about = "A lightweight Solana TUI wallet for Linux")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the TUI interface
    Tui,
    /// Create a new wallet (generates a new seed phrase)
    Create {
        #[arg(short, long)]
        label: String,
    },
    /// Import a wallet from a seed phrase or private key
    Import {
        #[arg(short, long)]
        label: String,
        #[arg(short, long, value_enum, default_value = "seed")]
        method: ImportMethod,
    },
    /// List all wallets
    List,
    /// Remove a wallet (deletes encrypted keystore file)
    Remove {
        /// Wallet to remove (pubkey or label)
        #[arg(short, long)]
        wallet: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
    /// Show balances for a wallet
    Balance {
        /// Wallet pubkey (or label — will be resolved)
        #[arg(short, long)]
        wallet: String,
    },
    /// Send tokens to an address
    Send {
        /// Wallet to send from (pubkey or label)
        #[arg(short, long)]
        wallet: String,
        /// Token mint or symbol (SOL, USDC, USDT, or full mint address)
        #[arg(short, long)]
        token: String,
        /// Amount to send (in human-readable units, e.g. 0.5)
        #[arg(short, long)]
        amount: f64,
        /// Recipient address (or address book label)
        #[arg(short = 'r', long)]
        to: String,
        /// Token decimals (override auto-detection)
        #[arg(long = "decimals", default_value = "0")]
        token_decimals: u8,
    },
    /// Show your wallet address for receiving
    Receive {
        /// Wallet to receive to (pubkey or label)
        #[arg(short, long)]
        wallet: String,
    },
    /// Save your passphrase to the OS keyring for auto-unlock
    Unlock,
    /// Remove saved passphrase from the OS keyring
    Lock,
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage the address book
    Contacts {
        #[command(subcommand)]
        action: ContactsAction,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum ImportMethod {
    /// Import from a BIP39 seed phrase
    Seed,
    /// Import from a base58 private key (Phantom export)
    Key,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set the RPC URL (Helius or any Solana RPC)
    SetRpc { url: String },
    /// Set the Jupiter API key (optional)
    SetJupiterKey { key: String },
    /// Show current config
    Show,
}

#[derive(Subcommand)]
enum ContactsAction {
    /// Add a contact
    Add {
        #[arg(short, long)]
        label: String,
        #[arg(short, long)]
        address: String,
        #[arg(short, long)]
        note: Option<String>,
    },
    /// List all contacts
    List,
    /// Remove a contact by label
    Remove { label: String },
}

fn resolve_wallet(query: &str) -> Result<String> {
    let wallets = wallet::list_wallets()?;
    // Exact pubkey match
    for w in &wallets {
        if w.pubkey == query {
            return Ok(w.pubkey.clone());
        }
    }
    // Label match (case-insensitive)
    for w in &wallets {
        if w.label.eq_ignore_ascii_case(query) {
            return Ok(w.pubkey.clone());
        }
    }
    // Assume it's a pubkey we don't have stored
    Ok(query.to_string())
}

/// Resolve a token identifier (SOL, USDC, USDT, or mint address) to (mint, decimals).
async fn resolve_token(token: &str, decimals_override: u8, wallet_pubkey: &str) -> Result<(String, u8)> {
    // Known tokens
    let known: &[(&str, &str, u8)] = &[
        ("SOL", "So11111111111111111111111111111111111111112", 9),
        ("USDC", "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", 6),
        ("USDT", "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", 6),
    ];

    // Check known symbols
    for (sym, mint, dec) in known {
        if token.eq_ignore_ascii_case(sym) || token == *mint {
            let decimals = if decimals_override > 0 { decimals_override } else { *dec };
            return Ok((mint.to_string(), decimals));
        }
    }

    // If it looks like a mint address, fetch its decimals from chain
    if token.len() > 30 {
        let rpc = rpc::Rpc::from_config()?;
        let balances = rpc::balance::fetch_balances(&rpc, wallet_pubkey).await?;
        // Look for this mint in the wallet's tokens
        for t in &balances.tokens {
            if t.mint == token {
                let decimals = if decimals_override > 0 { decimals_override } else { t.decimals };
                return Ok((token.to_string(), decimals));
            }
        }
        // Not in wallet — fetch mint info directly
        let mint_pk: solana_pubkey::Pubkey = token
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid mint address: {}", e))?;
        let account = rpc
            .client
            .get_account(&mint_pk)
            .await
            .map_err(|e| anyhow::anyhow!("Mint account not found: {}", e))?;
        // Parse mint decimals from account data (offset 44, 1 byte)
        if account.data.len() >= 45 {
            let dec = account.data[44];
            let decimals = if decimals_override > 0 { decimals_override } else { dec };
            return Ok((token.to_string(), decimals));
        }
        return Err(anyhow::anyhow!("Invalid mint account data"));
    }

    Err(anyhow::anyhow!(
        "Unknown token '{}'. Use SOL, USDC, USDT, or a full mint address.",
        token
    ))
}

/// Resolve a recipient from address book or use as raw address.
fn resolve_recipient(query: &str) -> Result<String> {
    // Try address book lookup
    let book = addressbook::AddressBook::load()?;
    if let Some(contact) = book.find(query) {
        return Ok(contact.address.clone());
    }
    // Validate as pubkey
    query
        .parse::<solana_pubkey::Pubkey>()
        .map(|_| query.to_string())
        .map_err(|_| anyhow::anyhow!("Invalid recipient address: {}", query))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Tui => {
            tui::run()
        }
        Commands::Balance { wallet: wallet_query } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let pubkey = resolve_wallet(&wallet_query)?;
                let rpc = rpc::Rpc::from_config()?;
                let price_svc = price::PriceService::from_config()?;

                println!("Fetching balances for {}...", pubkey);

                let mut balances = rpc::balance::fetch_balances(&rpc, &pubkey).await?;

                let mints: Vec<String> = balances.tokens.iter().map(|t| t.mint.clone()).collect();
                let prices = price_svc.get_prices(&mints).await.unwrap_or_default();
                rpc::balance::enrich_with_prices(&mut balances, &prices);

                println!();
                println!("{:<10} {:<12} {:<18} {:>14}", "Token", "Price", "Balance", "USD Value");
                println!("{}", "-".repeat(56));
                println!(
                    "{:<10} {:>12} {:<18} {:>14}",
                    "SOL",
                    balances.sol_usd_price.map(|p| format!("${:.4}", p)).unwrap_or("-".to_string()),
                    format!("{:.4}", balances.sol_balance),
                    balances.sol_usd_value.map(|v| format!("${:.2}", v)).unwrap_or("-".to_string()),
                );
                for t in &balances.tokens {
                    println!(
                        "{:<10} {:>12} {:<18} {:>14}",
                        t.symbol,
                        t.usd_price.map(|p| format!("${:.4}", p)).unwrap_or("-".to_string()),
                        format!("{:.4}", t.amount),
                        t.usd_value.map(|v| format!("${:.2}", v)).unwrap_or("-".to_string()),
                    );
                }
                println!("{}", "-".repeat(56));
                if let Some(total) = balances.total_usd_value {
                    println!("Total USD: ${:.2}", total);
                }
                Ok(())
            })
        }
        Commands::Send {
            wallet: wallet_query,
            token,
            amount,
            to,
            token_decimals,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let pubkey = resolve_wallet(&wallet_query)?;

                // Unlock the wallet — try keyring first, then prompt
                let passphrase = match keyring_helper::get_passphrase()? {
                    Some(p) => {
                        eprintln!("(unlocked via keyring)");
                        p
                    }
                    None => wallet::prompt_passphrase_unlock()?,
                };
                let unlocked = wallet::unlock_wallet(&pubkey, &passphrase)?;
                println!("Wallet unlocked: {} ({})", unlocked.label, unlocked.pubkey);

                // Resolve token mint
                let (mint, decimals) =
                    resolve_token(&token, token_decimals, &unlocked.pubkey).await?;

                // Resolve recipient (address book or raw address)
                let recipient = resolve_recipient(&to)?;

                println!(
                    "Sending {} {} to {}...",
                    amount, token, recipient
                );

                // Confirm
                print!("Confirm? (y/N): ");
                io::stdout().flush()?;
                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Cancelled.");
                        return Ok(());
                    }

                    let rpc = rpc::Rpc::from_config()?;
                    let signature = rpc::transfer::transfer(
                        &rpc,
                        &unlocked.keypair,
                        &recipient,
                        &mint,
                        amount,
                        decimals,
                    )
                    .await?;

                    println!();
                    println!("Transaction sent!");
                    println!("  Signature: {}", signature);
                    println!("  Explorer:  https://solscan.io/tx/{}", signature);
                    Ok(())
                })
        }
        Commands::Receive { wallet: wallet_query } => {
            let pubkey = resolve_wallet(&wallet_query)?;

            // Try to find label
            let wallets = wallet::list_wallets()?;
            let label = wallets
                .iter()
                .find(|w| w.pubkey == pubkey)
                .map(|w| w.label.clone())
                .unwrap_or_default();

            println!();
            println!("Wallet: {}", if label.is_empty() { &pubkey } else { &label });
            println!("Address: {}", pubkey);
            println!();
            println!("Share this address to receive SOL or SPL tokens.");
            println!("Explorer: https://solscan.io/account/{}", pubkey);
            Ok(())
        }
        Commands::Unlock => {
            let passphrase = wallet::prompt_passphrase_unlock()?;
            keyring_helper::set_passphrase(&passphrase)?;
            println!("Passphrase saved to OS keyring. Future commands will auto-unlock.");
            println!("Run 'fatwallet lock' to remove it.");
            Ok(())
        }
        Commands::Lock => {
            keyring_helper::delete_passphrase()?;
            println!("Passphrase removed from OS keyring.");
            Ok(())
        }
        Commands::Create { label } => {
            config::Config::ensure_dirs()?;
            let passphrase = wallet::prompt_passphrase(true)?;
            let (info, mnemonic) = wallet::create_wallet(&label, &passphrase)?;
            println!("\nWallet created successfully!");
            println!("  Label:   {}", info.label);
            println!("  Address: {}", info.pubkey);
            println!();
            println!("  === SEED PHRASE (SAVE THIS SECURELY) ===");
            println!("  {}", mnemonic);
            println!("  ==========================================");
            println!();
            println!("This seed phrase is the ONLY way to recover this wallet.");
            println!("It has been encrypted and stored. The phrase will NOT be shown again.");
            Ok(())
        }
        Commands::Import { label, method } => {
            config::Config::ensure_dirs()?;
            let passphrase = wallet::prompt_passphrase(true)?;
            match method {
                ImportMethod::Seed => {
                    print!("Enter seed phrase: ");
                    io::stdout().flush()?;
                    let seed_phrase = io::stdin()
                        .lock()
                        .lines()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("No input"))??;
                    let info =
                        wallet::import_from_seed_phrase(&seed_phrase, &passphrase, &label, 0, 0)?;
                    println!("\nWallet imported successfully!");
                    println!("  Label:   {}", info.label);
                    println!("  Address: {}", info.pubkey);
                }
                ImportMethod::Key => {
                    let private_key = rpassword::prompt_password("Enter private key (base58): ")?;
                    let info = wallet::import_from_private_key(&private_key, &passphrase, &label)?;
                    println!("\nWallet imported successfully!");
                    println!("  Label:   {}", info.label);
                    println!("  Address: {}", info.pubkey);
                }
            }
            Ok(())
        }
        Commands::List => {
            let wallets = wallet::list_wallets()?;
            if wallets.is_empty() {
                println!("No wallets found. Use 'fatwallet create' or 'fatwallet import' to add one.");
                return Ok(());
            }
            println!("{:<36} {:<20} {}", "Address", "Label", "Created");
            println!("{}", "-".repeat(80));
            for w in wallets {
                let created = chrono::DateTime::from_timestamp(w.created_at, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                println!("{:<36} {:<20} {}", w.pubkey, w.label, created);
            }
            Ok(())
        }
        Commands::Remove { wallet: wallet_query, yes } => {
            let pubkey = resolve_wallet(&wallet_query)?;
            let wallets = wallet::list_wallets()?;
            let info = wallets.iter().find(|w| w.pubkey == pubkey);

            if !yes {
                println!(
                    "  Wallet:  {}",
                    info.map(|w| w.label.as_str()).unwrap_or("(unknown)")
                );
                println!("  Address: {}", pubkey);
                println!();
                print!("Type the wallet label to confirm removal: ");
                io::stdout().flush()?;
                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    let label = info.map(|w| w.label.as_str()).unwrap_or("");
                    if input.trim() != label {
                        println!("Cancelled — label did not match.");
                        return Ok(());
                    }
                }

                wallet::remove_wallet(&pubkey)?;
                println!("Wallet {} removed.", pubkey);
                Ok(())
            }
        Commands::Config { action } => match action {
            ConfigAction::SetRpc { url } => {
                let mut cfg = config::Config::load()?;
                cfg.rpc_url = url;
                cfg.save()?;
                println!("RPC URL saved.");
                Ok(())
            }
            ConfigAction::SetJupiterKey { key } => {
                let mut cfg = config::Config::load()?;
                cfg.jupiter_api_key = key;
                cfg.save()?;
                println!("Jupiter API key saved.");
                Ok(())
            }
            ConfigAction::Show => {
                let cfg = config::Config::load()?;
                println!("{:<25} {}", "RPC URL:", cfg.rpc_url);
                println!("{:<25} {}", "Jupiter API URL:", cfg.jupiter_api_url);
                println!(
                    "{:<25} {}",
                    "Jupiter API Key:",
                    if cfg.jupiter_api_key.is_empty() {
                        "(not set)"
                    } else {
                        "********"
                    }
                );
                println!("{:<25} {}s", "Refresh interval:", cfg.refresh_interval_secs);
                Ok(())
            }
        },
        Commands::Contacts { action } => match action {
            ContactsAction::Add {
                label,
                address,
                note,
            } => {
                let mut book = addressbook::AddressBook::load()?;
                book.add(&label, &address, note)?;
                println!("Contact '{}' added.", label);
                Ok(())
            }
            ContactsAction::List => {
                let book = addressbook::AddressBook::load()?;
                if book.list().is_empty() {
                    println!("No contacts found.");
                    return Ok(());
                }
                println!("{:<20} {:<44} {}", "Label", "Address", "Note");
                println!("{}", "-".repeat(80));
                for c in book.list() {
                    println!(
                        "{:<20} {:<44} {}",
                        c.label,
                        c.address,
                        c.note.as_deref().unwrap_or("-")
                    );
                }
                Ok(())
            }
            ContactsAction::Remove { label } => {
                let mut book = addressbook::AddressBook::load()?;
                if book.remove(&label)? {
                    println!("Contact '{}' removed.", label);
                } else {
                    println!("Contact '{}' not found.", label);
                }
                Ok(())
            }
        },
    }
}