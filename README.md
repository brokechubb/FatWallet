# FatWallet

A lightweight Solana TUI wallet. Built in Rust with ratatui, encrypted-at-rest key storage, auto-discovery of SPL tokens (including Token-2022), USD price display, and gasless swaps via Jupiter Ultra.

## Features

- **Multiple wallets** — create, import, remove, and switch between wallets
- **Encrypted at rest** — Argon2id KDF + AES-256-GCM, per-wallet encrypted JSON files
- **BIP44 standard** — `m/44'/501'/0'/0'` derivation (Phantom/Solflare compatible)
- **Import via seed phrase or private key** — base58 key import supported
- **Auto-unlock via OS keyring** — save passphrase to OS keyring, no repeated typing
- **Auto-discover SPL tokens** — any token in your wallet is shown automatically, including Token-2022
- **USD prices** — Jupiter Price API v3, 60s cache
- **Token metadata** — unknown token symbols fetched via Helius DAS and cached locally
- **Transaction history** — concurrent fetch, scrollable list with direction, amount, counterparty
- **Transaction details** — full signature, fee, slot, explorer link, open in browser
- **Send SOL & SPL tokens** — auto-creates recipient ATA if needed, dynamic priority fees via `getRecentPrioritizationFees`, blockhash retry on expiry
- **Gasless swaps** — Jupiter Ultra API with quote validation, API key support
- **Address book** — save contacts for quick sending, confirmation before delete
- **Amount shortcuts** — type `all` or `max` for full balance, `$10` for USD amounts
- **Cross-platform** — Linux, macOS, Windows (keyring, TUI, crypto all cross-platform)
- **Responsive TUI** — adapts to terminal size, help screen with all keybindings

## Quick Start

```bash
# Build
cargo build --release

# Launch the TUI (defaults to TUI if no subcommand given)
./target/release/fatwallet

# Or use a subcommand
./target/release/fatwallet tui
```

### First-time Setup

1. **Configure your RPC URL** (Helius recommended for DAS token metadata support):
   ```bash
   fatwallet config set-rpc "https://mainnet.helius-rpc.com/?api-key=YOUR_KEY"
   ```
   Get a free API key at [helius.dev](https://www.helius.dev) — the free plan includes
   1M credits/month, which is plenty for personal use. Token symbol discovery requires
   Helius DAS support; other RPC providers (e.g. `api.mainnet-beta.solana.com`) work
   but will show truncated mint addresses instead of token symbols.

2. **Create a wallet:**
   ```bash
   fatwallet create --label main
   ```
   Save the seed phrase shown — it's the only way to recover your wallet.

3. **Or import an existing wallet:**
   ```bash
   # From seed phrase
   fatwallet import --label main --method seed

   # From base58 private key (Phantom export)
   fatwallet import --label main --method key
   ```

4. **Save passphrase to keyring (optional, for auto-unlock):**
   ```bash
   fatwallet unlock
   ```
   Future commands will auto-unlock without prompting.

5. **Launch the TUI:**
   ```bash
   fatwallet
   ```

## CLI Commands

```
fatwallet                          Launch TUI (default)
fatwallet tui                      Launch TUI
fatwallet create --label <name>    Create new wallet
fatwallet import --label <name>    Import wallet (--method seed|key)
fatwallet list                     List all wallets
fatwallet remove --wallet <name>   Remove a wallet
fatwallet balance --wallet <name>  Show balances + USD
fatwallet send                     Send tokens
  -w <wallet>  -t <SOL|USDC|USDT>  -a <amount>  -r <address|label>
fatwallet receive --wallet <name>  Show receive address
fatwallet unlock                   Save passphrase to OS keyring
fatwallet lock                     Remove passphrase from keyring
fatwallet config set-rpc <url>     Set RPC URL
fatwallet config set-jupiter-key <key>  Set Jupiter API key
fatwallet config show              Show current config
fatwallet contacts add             Add address book contact
  --label <name> --address <addr> [--note <text>]
fatwallet contacts list            List contacts
fatwallet contacts remove <label>  Remove contact
```

## TUI Keybindings

| Key | Action |
|-----|--------|
| `s` | Send tokens |
| `R` | Receive — show your address |
| `x` | Swap (gasless via Jupiter) |
| `i` | Import wallet from seed phrase |
| `k` | Import wallet from private key |
| `n` | Create new wallet |
| `a` | Address book |
| `r` | Force refresh balances & transactions |
| `Tab` | Next wallet |
| `1-9` | Switch to wallet by index |
| `Up/Dn` | Scroll transaction list |
| `Enter` | Open transaction detail |
| `h` | Help screen |
| `q` | Quit |

### Transaction Detail

| Key | Action |
|-----|--------|
| `Enter`/`Esc` | Back to dashboard |
| `c` | Copy signature to clipboard |
| `o` | Open in browser (Solscan) |

### Address Book

| Key | Action |
|-----|--------|
| `a` | Add new contact |
| `d` | Delete selected (confirms) |
| `s` / `Enter` | Send to selected contact |
| `c` | Copy selected address to clipboard |
| `Up/Dn` | Select contact |
| `Esc` | Back to dashboard |

### Send / Swap Amount Field

| Input | Meaning |
|-------|---------|
| `all` / `max` | Send full balance (SOL reserves 0.01 for gas) |
| `$10` | USD amount (converted to token units) |
| `1.5` | Raw token amount |

## Configuration

Config is stored at `~/.config/fatwallet/config.toml`:

```toml
rpc_url = "https://api.mainnet-beta.solana.com"
jupiter_api_url = "https://api.jup.ag/price/v3"
jupiter_api_key = ""
refresh_interval_secs = 30
```

For token metadata (symbol discovery), a Helius RPC URL is recommended as it supports the DAS `getAssetBatch` method.

### File Layout

```
~/.config/fatwallet/
├── config.toml              # RPC URL, Jupiter settings
├── contacts.toml            # Address book
├── token_cache.json         # Cached token metadata
└── wallets/
    ├── <pubkey1>.json       # Encrypted keystore
    └── <pubkey2>.json
```

## Security

- Private keys encrypted with Argon2id (64 MiB, 3 iterations) + AES-256-GCM
- Passphrase never stored on disk — only in OS keyring (if enabled)
- Keystore files have `0600` permissions
- Decrypted plaintext buffers zeroized from memory
- Unlocked keypairs zeroized on drop
- No hardcoded API keys in binary — user provides their own RPC URL
- Seed phrases use standard BIP44 `m/44'/501'/0'/0'` (Phantom-compatible)

## Cross-Platform

FatWallet builds and runs on Linux, macOS, and Windows:

```bash
# Linux/macOS (native)
cargo build --release

# Cross-compile to Windows from Linux
cargo build --release --target x86_64-pc-windows-gnu

# Native on Windows (requires Visual Studio Build Tools)
cargo build --release --target x86_64-pc-windows-msvc
```

OS keyring integration works on all platforms (GNOME Keyring, macOS Keychain, Windows Credential Manager).

## Dependencies

- [ratatui](https://ratatui.rs) + crossterm — TUI framework
- [solana-sdk](https://github.com/anza-xyz/solana-sdk) v4 — Solana Rust SDK
- [bip39](https://github.com/rust-bitcoin/rust-bip39) — mnemonic generation
- [argon2](https://github.com/RustCrypto/password-hashes) + [aes-gcm](https://github.com/RustCrypto/AEADs) — encryption
- [keyring](https://github.com/open-source-cooperative/keyring-rs) — OS keyring integration
- [Jupiter Price API](https://jup.ag) — token prices
- [Jupiter Ultra](https://lite-api.jup.ag) — gasless swaps
- [Helius DAS](https://www.helius.dev) — token metadata (optional, for symbol discovery)

## License

MIT