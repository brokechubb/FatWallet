use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, TableState},
    Frame,
};

use crate::app::state::{AppState, ImportMode, RefreshState, TxPendingKind, UIMode};
use crate::rpc::transactions::TxDirection;

pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();

    // On very small terminals, just show a message
    if area.width < 40 || area.height < 10 {
        let msg = Paragraph::new("Terminal too small (need 40x10 min)")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Red));
        frame.render_widget(msg, area);
        return;
    }

    // Main layout: header | content | footer
    // Header content lines: wallet tabs + address + refresh + status/pending + 2 border rows
    let tx_pending = state.tx_pending_kind.is_some();
    let has_status = !tx_pending && state.status_message.is_some();
    let header_h = if tx_pending || has_status { 6 } else { 5 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, state, chunks[0]);

    match state.ui_mode {
        UIMode::TxDetail => render_tx_detail(frame, state, chunks[1]),
        UIMode::Import => render_import(frame, state, chunks[1]),
        UIMode::Receive => render_receive(frame, state, chunks[1]),
        UIMode::Send => render_send(frame, state, chunks[1]),
        UIMode::Contacts => render_contacts(frame, state, chunks[1]),
        UIMode::ContactAdd => render_contact_add(frame, state, chunks[1]),
        UIMode::Help => render_help(frame, state, chunks[1]),
        UIMode::Swap => render_swap(frame, state, chunks[1]),
        _ => render_content(frame, state, chunks[1]),
    }

    render_footer(frame, state, chunks[2]);
}

fn render_header(frame: &mut Frame, state: &AppState, area: Rect) {
    let header_block = Block::default()
        .borders(Borders::ALL)
        .title(" FatWallet ")
        .title_alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan));

    // Wallet tabs — wrap if narrow
    let wallet_line = if state.wallets.is_empty() {
        Line::from(vec![Span::styled(
            "No wallets. Press [i] to import or [n] to create.",
            Style::default().fg(Color::DarkGray),
        )])
    } else {
        let spans: Vec<Span> = state
            .wallets
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let style = if i == state.active_wallet_idx {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Span::styled(format!(" [{}] {} ", i + 1, w.label), style)
            })
            .collect();
        Line::from(spans)
    };

    // Active wallet address — truncate if narrow
    let addr_line = if let Some(w) = state.active_wallet() {
        let addr = if area.width > 50 {
            w.pubkey.clone()
        } else {
            short_addr(&w.pubkey)
        };
        Line::from(vec![
            Span::styled("Address: ", Style::default().fg(Color::DarkGray)),
            Span::styled(addr, Style::default().fg(Color::Green)),
        ])
    } else {
        Line::from("")
    };

    // Refresh status
    let refresh_line = match state.refresh_state {
        RefreshState::Loading => Line::from(vec![Span::styled(
            "Loading...",
            Style::default().fg(Color::Yellow),
        )]),
        RefreshState::Error => Line::from(vec![Span::styled(
            format!(
                "Error: {}",
                state.last_error.as_deref().unwrap_or("unknown")
            ),
            Style::default().fg(Color::Red),
        )]),
        RefreshState::Idle => Line::from(""),
    };

    // Status message (transient notifications) — suppressed while a tx is pending,
    // since the pending line takes its place.
    let status_line = if state.tx_pending_kind.is_none() {
        if let Some(ref msg) = state.status_message {
            let color = if msg.contains("failed") || msg.contains("Failed") || msg.contains("Error") || msg.contains("error") || msg.starts_with("✗") {
                Color::Red
            } else if msg.contains("Sent!") || msg.contains("Swap complete!") || msg.starts_with("✓") || msg.contains("added") || msg.contains("removed") || msg.contains("copied") {
                Color::Green
            } else {
                Color::Blue
            };
            Line::from(vec![Span::styled(msg, Style::default().fg(color))])
        } else {
            Line::from("")
        }
    } else {
        Line::from("")
    };

    // Pending transaction indicator — prominent, animated, always shown while in flight.
    let pending_line = if let Some(kind) = state.tx_pending_kind {
        let spinner = if let Some(start) = state.tx_pending_start {
            let elapsed = start.elapsed().as_secs();
            match elapsed % 4 {
                0 => "[|]",
                1 => "[/]",
                2 => "[-]",
                _ => "[\\]",
            }
        } else {
            "[ ]"
        };
        let label = match kind {
            TxPendingKind::Send => "SEND IN PROGRESS",
            TxPendingKind::Swap => "SWAP IN PROGRESS",
        };
        let stage = state.tx_pending_stage.as_deref().unwrap_or("Working...");
        let elapsed_str = state.tx_pending_start
            .map(|s| {
                let e = s.elapsed().as_secs();
                if e < 60 { format!("{}s", e) } else { format!("{}m{:02}s", e / 60, e % 60) }
            })
            .unwrap_or_default();
        Line::from(vec![
            Span::styled(format!(" {} ", spinner), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} ", label), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(stage, Style::default().fg(Color::White)),
            Span::styled(format!("  {}", elapsed_str), Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from("")
    };

    let mut content = vec![wallet_line, addr_line];
    if area.height > 3 {
        content.push(refresh_line);
    }
    if !pending_line.spans.is_empty() {
        content.push(pending_line);
    } else if !status_line.spans.is_empty() {
        content.push(status_line);
    }

    let p = Paragraph::new(content).block(header_block);
    frame.render_widget(p, area);
}

fn render_content(frame: &mut Frame, state: &AppState, area: Rect) {
    if state.wallets.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "No wallets found.",
                Style::default().fg(Color::Red),
            )]),
            Line::from(""),
            Line::from("Press [n] to create a new wallet"),
            Line::from("Press [i] to import an existing wallet"),
            Line::from("Press [q] to quit"),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(msg, area);
        return;
    }

    // Split content: balances (top) | transactions (bottom)
    // Balance section size adapts to number of tokens
    let balance_rows = state
        .balances
        .as_ref()
        .map(|b| 2 + b.count()) // header + border + rows
        .unwrap_or(3);
    let balance_h = ((balance_rows + 2) as u16).min(area.height / 2).max(4);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(balance_h),
            Constraint::Min(3),
        ])
        .split(area);

    render_balances(frame, state, chunks[0]);
    render_transactions(frame, state, chunks[1]);
}

fn render_balances(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Balances ")
        .style(Style::default().fg(Color::White));

    if let Some(ref balances) = state.balances {
        let mut rows: Vec<Row> = Vec::new();

        // SOL row
        rows.push(Row::new(vec![
            "SOL".to_string(),
            format!("{:.6}", balances.sol_balance),
            balances.sol_usd_price.map(|p| format!("${:.4}", p)).unwrap_or("-".to_string()),
            balances.sol_usd_value.map(|v| format!("${:.2}", v)).unwrap_or("-".to_string()),
        ]));

        // Token rows
        for t in &balances.tokens {
            rows.push(Row::new(vec![
                t.symbol.clone(),
                t.ui_amount_string.clone(),
                t.usd_price.map(|p| format!("${:.6}", p)).unwrap_or("-".to_string()),
                t.usd_value.map(|v| format!("${:.2}", v)).unwrap_or("-".to_string()),
            ]));
        }

        // Total row
        let total_str = balances.total_usd_value.map(|v| format!("${:.2}", v)).unwrap_or("-".to_string());
        rows.push(Row::new(vec!["Total".to_string(), String::new(), String::new(), total_str])
            .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Green)));

        // Column widths scale with terminal width
        let w = area.width;
        let col_widths = balance_column_widths(w);

        let table = Table::new(rows, col_widths)
            .header(
                Row::new(vec!["Token", "Balance", "Price", "USD Value"])
                    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            )
            .block(block);

        frame.render_widget(table, area);
    } else {
        let msg = Paragraph::new("Loading balances...").block(block);
        frame.render_widget(msg, area);
    }
}

fn render_transactions(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Transactions ")
        .style(Style::default().fg(Color::White));

    if state.transactions.is_empty() {
        let msg = if state.refresh_state == RefreshState::Loading {
            "Loading transactions..."
        } else {
            "No transactions found. Press [r] to refresh."
        };
        let p = Paragraph::new(msg).block(block);
        frame.render_widget(p, area);
        return;
    }

    let rows: Vec<Row> = state
        .transactions
        .iter()
        .map(|tx| {
            let time = tx
                .block_time
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|dt| dt.format("%m-%d %H:%M").to_string())
                .unwrap_or("??".to_string());

            let dir_style = match tx.direction {
                TxDirection::Incoming => Style::default().fg(Color::Green),
                TxDirection::Outgoing => Style::default().fg(Color::Red),
                TxDirection::Unknown => Style::default().fg(Color::Gray),
            };

            let arrow = match tx.direction {
                TxDirection::Incoming => "+",
                TxDirection::Outgoing => "-",
                TxDirection::Unknown => " ",
            };

            let amount = tx
                .amount
                .map(|a| {
                    let formatted = if a.abs() >= 1000.0 {
                        format!("{:.2}", a)
                    } else if a.abs() >= 1.0 {
                        format!("{:.4}", a)
                    } else {
                        format!("{:.6}", a)
                    };
                    format!("{}{} {}", arrow, formatted, tx.token_symbol)
                })
                .unwrap_or("-".to_string());

            let counterparty = tx
                .counterparty
                .as_ref()
                .map(|c| c.clone())
                .unwrap_or("-".to_string());

            Row::new(vec![time, counterparty, amount]).style(dir_style)
        })
        .collect();

    // Column widths scale with terminal width
    let w = area.width;
    let col_widths = tx_column_widths(w);

    let table = Table::new(rows, col_widths)
        .header(
            Row::new(vec!["Time", "Counterparty", "Amount"])
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .block(block);

    let mut table_state = TableState::default();
    table_state.select(Some(state.tx_scroll));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_tx_detail(frame: &mut Frame, state: &AppState, area: Rect) {
    let idx = match state.tx_detail_idx {
        Some(i) => i,
        None => return,
    };

    let tx = match state.transactions.get(idx) {
        Some(t) => t,
        None => return,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Transaction Details ")
        .style(Style::default().fg(Color::Cyan));

    let dir_str = match tx.direction {
        TxDirection::Incoming => "Incoming",
        TxDirection::Outgoing => "Outgoing",
        TxDirection::Unknown => "Unknown",
    };

    let dir_color = match tx.direction {
        TxDirection::Incoming => Color::Green,
        TxDirection::Outgoing => Color::Red,
        TxDirection::Unknown => Color::Gray,
    };

    let time = tx
        .block_time
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or("Unknown".to_string());

    let amount = tx
        .amount
        .map(|a| format!("{:.9} {}", a, tx.token_symbol))
        .unwrap_or("-".to_string());

    let fee = tx
        .fee
        .map(|f| format!("{} lamports ({:.9} SOL)", f, f as f64 / 1_000_000_000.0))
        .unwrap_or("-".to_string());

    let counterparty = tx.counterparty.clone().unwrap_or("-".to_string());

    let sig = &tx.signature;
    let explorer = format!("https://solscan.io/tx/{}", sig);

    // Inner width accounting for border padding
    let inner_w = area.width.saturating_sub(4) as usize;

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("Direction:   ", Style::default().fg(Color::DarkGray)),
        Span::styled(dir_str, Style::default().fg(dir_color).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled("Signature:", Style::default().fg(Color::DarkGray))]));
    for chunk in split_text(sig, inner_w) {
        lines.push(Line::from(vec![Span::styled(chunk, Style::default().fg(Color::Yellow))]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Time:        ", Style::default().fg(Color::DarkGray)),
        Span::styled(time, Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Amount:      ", Style::default().fg(Color::DarkGray)),
        Span::styled(amount, Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Fee:         ", Style::default().fg(Color::DarkGray)),
        Span::styled(fee, Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Counterparty:", Style::default().fg(Color::DarkGray)),
        Span::styled(counterparty.clone(), Style::default().fg(Color::White)),
    ]));
    // Counterparty continuation lines if it's too long
    if counterparty.len() > inner_w.saturating_sub(13) {
        for chunk in split_text(&counterparty[counterparty.len().min(inner_w.saturating_sub(13))..], inner_w) {
            lines.push(Line::from(vec![Span::styled(format!("  {}", chunk), Style::default().fg(Color::White))]));
        }
    }
    lines.push(Line::from(vec![
        Span::styled("Slot:        ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            tx.slot.map(|s| s.to_string()).unwrap_or("-".to_string()),
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Status:      ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if tx.err.is_some() { "Failed" } else { "Success" },
            Style::default().fg(if tx.err.is_some() { Color::Red } else { Color::Green }),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled("Explorer:", Style::default().fg(Color::DarkGray))]));
    for chunk in split_text(&explorer, inner_w) {
        lines.push(Line::from(vec![Span::styled(chunk, Style::default().fg(Color::Blue))]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "[Enter/Esc] back  [c] copy sig  [o] open in browser",
        Style::default().fg(Color::DarkGray),
    )]));

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_import(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(match state.import_mode {
            ImportMode::Create => " Create New Wallet ",
            ImportMode::ImportSeed => " Import from Seed Phrase ",
            ImportMode::ImportKey => " Import from Private Key ",
        })
        .style(Style::default().fg(Color::Cyan));

    let (prompt, show_input, is_masked) = match state.import_mode {
        ImportMode::Create => match state.input_step {
            0 => ("Enter passphrase (will be encrypted):", true, true),
            1 => ("Confirm passphrase:", true, true),
            2 => ("Enter wallet label:", true, false),
            _ => ("Done!", false, false),
        },
        ImportMode::ImportSeed => match state.input_step {
            0 => ("Enter passphrase (for encrypting):", true, true),
            1 => ("Confirm passphrase:", true, true),
            2 => ("Enter seed phrase (12 or 24 words):", true, false),
            3 => ("Enter wallet label:", true, false),
            _ => ("Done!", false, false),
        },
        ImportMode::ImportKey => match state.input_step {
            0 => ("Enter passphrase (for encrypting):", true, true),
            1 => ("Confirm passphrase:", true, true),
            2 => ("Enter base58 private key:", true, false),
            3 => ("Enter wallet label:", true, false),
            _ => ("Done!", false, false),
        },
    };

    let mut lines = vec![
        Line::from(vec![Span::styled(prompt, Style::default().fg(Color::White))]),
        Line::from(""),
    ];

    if show_input {
        let display = if is_masked {
            "*".repeat(state.input_field.len())
        } else {
            state.input_field.clone()
        };

        // Split long input across multiple lines to fit terminal width
        let inner_w = area.width.saturating_sub(6) as usize; // border + "> " + cursor
        let chunks = split_text(&display, inner_w);

        for (i, chunk) in chunks.iter().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled("> ", Style::default().fg(Color::Yellow)),
                    Span::styled(chunk.clone(), Style::default().fg(Color::White)),
                    if i == chunks.len() - 1 {
                        Span::styled("_", Style::default().fg(Color::DarkGray))
                    } else {
                        Span::raw("")
                    },
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default().fg(Color::Yellow)),
                    Span::styled(chunk.clone(), Style::default().fg(Color::White)),
                    if i == chunks.len() - 1 {
                        Span::styled("_", Style::default().fg(Color::DarkGray))
                    } else {
                        Span::raw("")
                    },
                ]));
            }
        }
    }

    if let Some(ref msg) = state.status_message {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(msg, Style::default().fg(Color::Red))]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "[Enter] next  [Esc] cancel",
        Style::default().fg(Color::DarkGray),
    )]));

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_receive(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Receive ")
        .style(Style::default().fg(Color::Cyan));

    let pubkey = state.active_pubkey().unwrap_or_default();
    let label = state.active_wallet().map(|w| w.label.clone()).unwrap_or_default();

    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            format!("Wallet: {}", label),
            Style::default().fg(Color::White),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled("Address: ", Style::default().fg(Color::DarkGray))]),
        Line::from(vec![Span::styled(&pubkey, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Share this address to receive SOL or SPL tokens.",
            Style::default().fg(Color::DarkGray),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "[c] copy to clipboard  [Esc] back",
            Style::default().fg(Color::DarkGray),
        )]),
    ];

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_send(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Send ")
        .style(Style::default().fg(Color::Cyan));

    let key_style = Style::default().fg(Color::DarkGray);
    let val_style = Style::default().fg(Color::White);
    let input_style = Style::default().fg(Color::Yellow);

    let (prompt, is_masked) = match state.input_step {
        0 => ("Token (SOL, USDC, USDT, or mint address):", false),
        1 => ("Amount ($10 for USD, or token amount):", false),
        2 => (if state.show_contact_picker { "Select recipient (Up/Dn to browse, Tab to type):" } else { "Recipient (address or label, Tab to browse):" }, false),
        3 => ("Passphrase:", true),
        4 => ("Confirm send? Press Enter to submit.", false),
        _ => ("Done!", false),
    };

    let mut lines = Vec::new();

    // Show completed steps
    if state.input_step > 0 {
        lines.push(Line::from(vec![Span::styled("Token:   ", key_style), Span::styled(&state.temp_token, val_style)]));
    }
    if state.input_step > 1 {
        lines.push(Line::from(vec![Span::styled("Amount:  ", key_style), Span::styled(&state.temp_amount, val_style)]));
    }
    if !state.temp_recipient.is_empty() && state.input_step > 1 {
        lines.push(Line::from(vec![Span::styled("To:      ", key_style), Span::styled(&state.temp_recipient, val_style)]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(prompt, val_style)]));
    lines.push(Line::from(""));

    // At recipient step, show contact list if picker is active
    if state.input_step == 2 && state.show_contact_picker {
        if let Some(ref book) = state.contacts {
            if book.list().is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    "No contacts in address book.",
                    Style::default().fg(Color::DarkGray),
                )]));
            } else {
                for (i, c) in book.list().iter().enumerate() {
                    let style = if i == state.contact_scroll {
                        Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let prefix = if i == state.contact_scroll { ">" } else { " " };
                    lines.push(Line::from(vec![
                        Span::styled(format!("{} {:<15} ", prefix, c.label), style),
                        Span::styled(c.address.clone(), if i == state.contact_scroll { style } else { Style::default().fg(Color::Green) }),
                    ]));
                }
            }
        }
    }

    if state.input_step < 4 && !(state.input_step == 2 && state.show_contact_picker) {
        let display = if is_masked {
            "*".repeat(state.input_field.len())
        } else {
            state.input_field.clone()
        };

        let inner_w = area.width.saturating_sub(6) as usize;
        let chunks = split_text(&display, inner_w);
        for (i, chunk) in chunks.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(if i == 0 { "> " } else { "  " }, input_style),
                Span::styled(chunk.clone(), val_style),
                if i == chunks.len() - 1 {
                    Span::styled("_", Style::default().fg(Color::DarkGray))
                } else {
                    Span::raw("")
                },
            ]));
        }
    }

    if let Some(ref msg) = state.status_message {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(msg, Style::default().fg(Color::Red))]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "[Enter] next  [Esc] cancel",
        Style::default().fg(Color::DarkGray),
    )]));

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_contacts(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Address Book ")
        .style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(ref book) = state.contacts {
        if book.list().is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "No contacts yet. Press [a] to add one.",
                Style::default().fg(Color::DarkGray),
            )]));
        } else {
            lines.push(Line::from(vec![Span::styled(
                "Contacts (Up/Dn to select):",
                Style::default().fg(Color::Cyan),
            )]));
            for (i, c) in book.list().iter().enumerate() {
                let style = if i == state.contact_scroll {
                    Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let prefix = if i == state.contact_scroll { ">" } else { " " };
                lines.push(Line::from(vec![
                    Span::styled(format!("{} {:<15} ", prefix, c.label), style),
                    Span::styled(c.address.clone(), if i == state.contact_scroll { style } else { Style::default().fg(Color::Green) }),
                ]));
            }
        }
    }

    if let Some(ref msg) = state.status_message {
        lines.push(Line::from(""));
        let color = if state.confirm_delete_contact { Color::Yellow } else { Color::Blue };
        lines.push(Line::from(vec![Span::styled(msg, Style::default().fg(color))]));
    }

    if !state.confirm_delete_contact {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "[a]dd  [d]elete  [s]end  [c]opy  [Enter] send  [Esc] back",
            Style::default().fg(Color::DarkGray),
        )]));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "[y] confirm delete  [n/Esc] cancel",
            Style::default().fg(Color::Yellow),
        )]));
    }

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_contact_add(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Add Contact ")
        .style(Style::default().fg(Color::Cyan));

    let prompt = match state.input_step {
        0 => "Enter contact label:",
        1 => "Enter contact address:",
        _ => "Done!",
    };

    let mut lines = vec![
        Line::from(vec![Span::styled(prompt, Style::default().fg(Color::White))]),
        Line::from(""),
    ];

    if state.input_step > 0 {
        lines.push(Line::from(vec![
            Span::styled("Label:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(&state.temp_label, Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(""));
    }

    let inner_w = area.width.saturating_sub(6) as usize;
    let display = state.input_field.clone();
    let chunks = split_text(&display, inner_w);
    for (i, chunk) in chunks.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(if i == 0 { "> " } else { "  " }, Style::default().fg(Color::Yellow)),
            Span::styled(chunk.clone(), Style::default().fg(Color::White)),
            if i == chunks.len() - 1 {
                Span::styled("_", Style::default().fg(Color::DarkGray))
            } else {
                Span::raw("")
            },
        ]));
    }

    if let Some(ref msg) = state.status_message {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(msg, Style::default().fg(Color::Red))]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "[Enter] next  [Esc] cancel",
        Style::default().fg(Color::DarkGray),
    )]));

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_swap(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Swap (Gasless via Jupiter Ultra) ")
        .style(Style::default().fg(Color::Cyan));

    let key_style = Style::default().fg(Color::DarkGray);
    let val_style = Style::default().fg(Color::White);
    let input_style = Style::default().fg(Color::Yellow);

    let (prompt, is_masked) = match state.input_step {
        0 => ("Token in (SOL, USDC, USDT):", false),
        1 => ("Token out (SOL, USDC, USDT):", false),
        2 => ("Amount ($10 for USD, or token amount):", false),
        3 => ("Passphrase:", true),
        4 => ("Confirm swap? Press Enter to submit.", false),
        _ => ("Done!", false),
    };

    let mut lines = Vec::new();

    if state.input_step > 0 {
        lines.push(Line::from(vec![Span::styled("From:    ", key_style), Span::styled(&state.temp_token, val_style)]));
    }
    if state.input_step > 1 {
        lines.push(Line::from(vec![Span::styled("To:      ", key_style), Span::styled(&state.temp_label, val_style)]));
    }
    if state.input_step > 2 {
        lines.push(Line::from(vec![Span::styled("Amount:  ", key_style), Span::styled(&state.temp_amount, val_style)]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(prompt, val_style)]));
    lines.push(Line::from(""));

    if state.input_step < 4 {
        let display = if is_masked {
            "*".repeat(state.input_field.len())
        } else {
            state.input_field.clone()
        };

        let inner_w = area.width.saturating_sub(6) as usize;
        let chunks = split_text(&display, inner_w);
        for (i, chunk) in chunks.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(if i == 0 { "> " } else { "  " }, input_style),
                Span::styled(chunk.clone(), val_style),
                if i == chunks.len() - 1 {
                    Span::styled("_", Style::default().fg(Color::DarkGray))
                } else {
                    Span::raw("")
                },
            ]));
        }
    }

    if let Some(ref msg) = state.status_message {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(msg, Style::default().fg(Color::Red))]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "[Enter] next  [Esc] cancel",
        Style::default().fg(Color::DarkGray),
    )]));

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

fn render_help(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .style(Style::default().fg(Color::Cyan));

    let key_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::White);
    let sec_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let lines = vec![
        Line::from(vec![Span::styled("Dashboard Keys", sec_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("  s       ", key_style), Span::styled("Send tokens", desc_style)]),
        Line::from(vec![Span::styled("  r       ", key_style), Span::styled("Force refresh balances & transactions", desc_style)]),
        Line::from(vec![Span::styled("  R       ", key_style), Span::styled("Receive — show your address", desc_style)]),
        Line::from(vec![Span::styled("  x       ", key_style), Span::styled("Gasless swap (Jupiter Ultra)", desc_style)]),
        Line::from(vec![Span::styled("  i       ", key_style), Span::styled("Import wallet from seed phrase", desc_style)]),
        Line::from(vec![Span::styled("  k       ", key_style), Span::styled("Import wallet from private key", desc_style)]),
        Line::from(vec![Span::styled("  n       ", key_style), Span::styled("Create new wallet", desc_style)]),
        Line::from(vec![Span::styled("  a       ", key_style), Span::styled("Address book — browse contacts", desc_style)]),
        Line::from(vec![Span::styled("  Tab     ", key_style), Span::styled("Next wallet", desc_style)]),
        Line::from(vec![Span::styled("  1-9     ", key_style), Span::styled("Switch to wallet by index", desc_style)]),
        Line::from(vec![Span::styled("  Up/Dn   ", key_style), Span::styled("Scroll transaction list", desc_style)]),
        Line::from(vec![Span::styled("  Enter   ", key_style), Span::styled("Open transaction detail", desc_style)]),
        Line::from(vec![Span::styled("  h       ", key_style), Span::styled("This help screen", desc_style)]),
        Line::from(vec![Span::styled("  q       ", key_style), Span::styled("Quit", desc_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("Transaction Detail", sec_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("  Enter   ", key_style), Span::styled("Back to dashboard", desc_style)]),
        Line::from(vec![Span::styled("  c       ", key_style), Span::styled("Copy signature to clipboard", desc_style)]),
        Line::from(vec![Span::styled("  o       ", key_style), Span::styled("Open in browser (Solscan)", desc_style)]),
        Line::from(vec![Span::styled("  Esc     ", key_style), Span::styled("Back to dashboard", desc_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("Address Book", sec_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("  a       ", key_style), Span::styled("Add new contact", desc_style)]),
        Line::from(vec![Span::styled("  d       ", key_style), Span::styled("Delete selected contact", desc_style)]),
        Line::from(vec![Span::styled("  s       ", key_style), Span::styled("Send to selected contact", desc_style)]),
        Line::from(vec![Span::styled("  c       ", key_style), Span::styled("Copy selected address", desc_style)]),
        Line::from(vec![Span::styled("  Enter   ", key_style), Span::styled("Send to selected contact", desc_style)]),
        Line::from(vec![Span::styled("  Up/Dn   ", key_style), Span::styled("Select contact", desc_style)]),
        Line::from(vec![Span::styled("  Esc     ", key_style), Span::styled("Back to dashboard", desc_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("Forms (Import, Send, Add Contact)", sec_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("  Enter   ", key_style), Span::styled("Confirm / next step", desc_style)]),
        Line::from(vec![Span::styled("  Esc     ", key_style), Span::styled("Cancel / back", desc_style)]),
        Line::from(vec![Span::styled("  Type    ", key_style), Span::styled("Enter text into fields", desc_style)]),
        Line::from(vec![Span::styled("  Bksp    ", key_style), Span::styled("Delete last character", desc_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("Amount Field", sec_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("  $10     ", key_style), Span::styled("USD amount (converted to token units)", desc_style)]),
        Line::from(vec![Span::styled("  1.5     ", key_style), Span::styled("Raw token amount", desc_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("Help Screen", sec_style)]),
        Line::from(""),
        Line::from(vec![Span::styled("  Up/Dn   ", key_style), Span::styled("Scroll this help", desc_style)]),
        Line::from(vec![Span::styled("  Esc     ", key_style), Span::styled("Close this help", desc_style)]),
    ];

    let total_lines = lines.len() as u16;
    let visible_height = area.height.saturating_sub(2); // minus borders
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = state.help_scroll.min(max_scroll);

    let p = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0));
    frame.render_widget(p, area);
}

fn render_footer(frame: &mut Frame, state: &AppState, area: Rect) {
    let key_style = Style::default().fg(Color::Yellow);
    let sep = Span::raw("  ");

    let line = match state.ui_mode {
        UIMode::Dashboard => {
            let left_spans = vec![
                Span::styled("[s]end", key_style), sep.clone(),
                Span::styled("[R]eceive", key_style), sep.clone(),
                Span::styled("[x]swap", key_style),
            ];
            let right_spans = vec![
                Span::styled("[h]elp", key_style), sep.clone(),
                Span::styled("[q]uit", key_style),
            ];

            let left_len: usize = left_spans.iter().map(|s| s.width()).sum();
            let right_len: usize = right_spans.iter().map(|s| s.width()).sum();
            let gap = area.width as usize - left_len - right_len;

            let mut spans = left_spans;
            if gap > 0 {
                spans.push(Span::raw(" ".repeat(gap)));
            }
            spans.extend(right_spans);
            Line::from(spans)
        }
        UIMode::TxDetail => {
            let left = vec![
                Span::styled("[Enter/Esc]back", key_style), sep.clone(),
                Span::styled("[c]opy sig", key_style), sep.clone(),
                Span::styled("[o]pen", key_style),
            ];
            let right = vec![Span::styled("[q]uit", key_style)];
            let left_len: usize = left.iter().map(|s| s.width()).sum();
            let right_len: usize = right.iter().map(|s| s.width()).sum();
            let gap = area.width as usize - left_len - right_len;
            let mut spans = left;
            if gap > 0 {
                spans.push(Span::raw(" ".repeat(gap)));
            }
            spans.extend(right);
            Line::from(spans)
        }
        UIMode::Receive => {
            Line::from(vec![
                Span::styled("[c]opy", key_style), sep.clone(),
                Span::styled("[Esc]back", key_style),
            ])
        }
        UIMode::Contacts => {
            Line::from(vec![
                Span::styled("[a]dd", key_style), sep.clone(),
                Span::styled("[d]elete", key_style), sep.clone(),
                Span::styled("[s]end", key_style), sep.clone(),
                Span::styled("[c]opy", key_style), sep.clone(),
                Span::styled("[Esc]back", key_style),
            ])
        }
        UIMode::ContactAdd => {
            Line::from(vec![
                Span::styled("[Enter]confirm", key_style), sep.clone(),
                Span::styled("[Esc]cancel", key_style),
            ])
        }
        UIMode::Import | UIMode::Send | UIMode::Swap => {
            Line::from(vec![
                Span::styled("[Enter]confirm", key_style), sep.clone(),
                Span::styled("[Esc]cancel", key_style),
            ])
        }
        UIMode::Help => {
            Line::from(vec![Span::styled("[Esc]back", key_style)])
        }
        _ => Line::from(vec![Span::styled("[Esc]back", key_style)]),
    };

    let p = Paragraph::new(line).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(p, area);
}

/// Compute balance table column widths proportional to terminal width.
fn balance_column_widths(w: u16) -> [Constraint; 4] {
    if w >= 70 {
        [
            Constraint::Length(12),
            Constraint::Min(15),
            Constraint::Length(14),
            Constraint::Length(14),
        ]
    } else if w >= 55 {
        [
            Constraint::Length(10),
            Constraint::Min(12),
            Constraint::Length(11),
            Constraint::Length(11),
        ]
    } else {
        [
            Constraint::Length(8),
            Constraint::Min(8),
            Constraint::Length(9),
            Constraint::Length(9),
        ]
    }
}

/// Compute transaction table column widths proportional to terminal width.
fn tx_column_widths(w: u16) -> [Constraint; 3] {
    if w >= 70 {
        [
            Constraint::Length(14),
            Constraint::Min(10),
            Constraint::Length(22),
        ]
    } else if w >= 50 {
        [
            Constraint::Length(11),
            Constraint::Min(8),
            Constraint::Length(18),
        ]
    } else {
        [
            Constraint::Length(10),
            Constraint::Min(5),
            Constraint::Length(15),
        ]
    }
}

fn split_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max_width).min(text.len());
        chunks.push(text[start..end].to_string());
        start = end;
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn short_addr(addr: &str) -> String {
    if addr.len() > 8 {
        format!("{}...{}", &addr[..4], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}