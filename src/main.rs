mod config;
mod models;
mod terminal_ui;
mod ui;

use config::get_user_config;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use futures_util::StreamExt;
use models::{
    App, OrderBook, ProfilingStats, RestDepthResponse, ServerTimeResponse, Trade, TradeHistory,
    WsAggTrade, WsDepthUpdate,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use terminal_ui::{restore_terminal, setup_terminal};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use ui::ui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (symbol, bucket_size) = get_user_config();
    let mut app = App::new(symbol.clone(), bucket_size);

    let stats = Arc::new(ProfilingStats::default());

    let order_book = Arc::new(RwLock::new(OrderBook::new()));
    let order_book_ws = Arc::clone(&order_book);
    let order_book_render = Arc::clone(&order_book);

    let trade_history = Arc::new(RwLock::new(TradeHistory::new(60_000)));
    let trade_history_ws = Arc::clone(&trade_history);
    let trade_history_render = Arc::clone(&trade_history);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(WsDepthUpdate, Instant)>();
    let (trade_tx, mut trade_rx) = tokio::sync::mpsc::unbounded_channel::<Trade>();

    let ws_url = format!(
        "wss://fstream.binance.com/stream?streams={}@depth@100ms/{}@aggTrade",
        symbol, symbol
    );

    println!("\x1b[33mConnecting to WebSocket...\x1b[0m");

    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (_write, mut read) = ws_stream.split();

    println!("\x1b[32mWebSocket connected! Buffering events...\x1b[0m");

    let tx_clone = tx.clone();
    let trade_tx_clone = trade_tx.clone();
    let stats_ws = Arc::clone(&stats);
    tokio::spawn(async move {
        while let Some(msg_result) = read.next().await {
            let recv_time = Instant::now();
            match msg_result {
                Ok(Message::Text(text)) => {
                    let parse_start = Instant::now();

                    if let Ok(wrapper) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(stream) = wrapper.get("stream").and_then(|s| s.as_str()) {
                            if let Some(data) = wrapper.get("data") {
                                if stream.contains("@depth") {
                                    if let Ok(update) =
                                        serde_json::from_value::<WsDepthUpdate>(data.clone())
                                    {
                                        let parse_elapsed = parse_start.elapsed().as_micros() as u64;
                                        stats_ws.record(
                                            &stats_ws.ws_parse_time,
                                            &stats_ws.ws_parse_count,
                                            parse_elapsed,
                                        );
                                        let _ = tx_clone.send((update, recv_time));
                                    }
                                } else if stream.contains("@aggTrade") {
                                    if let Ok(agg_trade) =
                                        serde_json::from_value::<WsAggTrade>(data.clone())
                                    {
                                        if let (Ok(price), Ok(qty)) = (
                                            agg_trade.price.parse::<f64>(),
                                            agg_trade.quantity.parse::<f64>(),
                                        ) {
                                            let trade = Trade {
                                                timestamp_ms: agg_trade.trade_time,
                                                price,
                                                quantity: qty,
                                                is_buy: !agg_trade.is_buyer_maker,
                                            };
                                            let _ = trade_tx_clone.send(trade);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(Message::Ping(_)) => {}
                Ok(Message::Close(_)) => {
                    eprintln!("\x1b[31mWebSocket closed by server\x1b[0m");
                    break;
                }
                Err(e) => {
                    eprintln!("\x1b[31mWebSocket error: {}\x1b[0m", e);
                    break;
                }
                _ => {}
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    println!("\x1b[33mFetching REST depth snapshot...\x1b[0m");

    let client = reqwest::Client::new();
    let rest_url = format!(
        "https://fapi.binance.com/fapi/v1/depth?symbol={}&limit=1000",
        symbol.to_uppercase()
    );

    let snapshot: RestDepthResponse = client.get(&rest_url).send().await?.json().await?;
    let snapshot_update_id = snapshot.last_update_id;

    println!(
        "\x1b[32mSnapshot received! lastUpdateId: {}\x1b[0m",
        snapshot_update_id
    );

    println!("\x1b[33mCalibrating clock offset...\x1b[0m");
    let time_url = "https://fapi.binance.com/fapi/v1/time";
    let local_before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let server_time: ServerTimeResponse = client.get(time_url).send().await?.json().await?;
    let local_after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let local_mid = (local_before + local_after) / 2;
    let clock_offset = local_mid - server_time.server_time as i64;
    println!(
        "\x1b[32mClock offset: {}ms (local {} Binance)\x1b[0m",
        clock_offset.abs(),
        if clock_offset > 0 {
            "ahead of"
        } else {
            "behind"
        }
    );

    {
        let mut book = order_book.write().await;
        book.apply_snapshot(&snapshot);
        book.clock_offset = clock_offset;
    }

    println!("\x1b[33mSyncing with buffered events...\x1b[0m");

    let mut synced = false;
    let mut last_final_update_id: u64 = 0;

    while let Ok((update, _recv_time)) = rx.try_recv() {
        if update.final_update_id < snapshot_update_id {
            continue;
        }

        if !synced {
            if update.first_update_id <= snapshot_update_id
                && update.final_update_id > snapshot_update_id
            {
                let mut book = order_book.write().await;
                book.apply_update(&update);
                last_final_update_id = update.final_update_id;
                synced = true;
                println!(
                    "\x1b[32mSynced! First valid update: U={}, u={}\x1b[0m",
                    update.first_update_id, update.final_update_id
                );
            }
            continue;
        }

        if update.prev_final_update_id == last_final_update_id {
            let mut book = order_book.write().await;
            book.apply_update(&update);
            last_final_update_id = update.final_update_id;
        }
    }

    if !synced {
        println!("\x1b[33mWaiting for sync event from live stream...\x1b[0m");
    }

    println!("\x1b[32mEntering live mode with ratatui!\x1b[0m");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut terminal = setup_terminal()?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_main = Arc::clone(&shutdown);

    let stats_main = Arc::clone(&stats);

    let render_interval = Duration::from_millis(50);
    let mut last_render = Instant::now();

    let mut cached_clock_offset = {
        let book = order_book.read().await;
        book.clock_offset
    };

    loop {
        if shutdown_main.load(Ordering::Relaxed) {
            break;
        }

        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key_event) = event::read()? {
                if key_event.modifiers.contains(KeyModifiers::CONTROL)
                    && key_event.code == KeyCode::Char('c')
                {
                    break;
                }
                if key_event.code == KeyCode::Char('q') {
                    break;
                }
            }
        }

        let mut pending_updates: Vec<(WsDepthUpdate, Instant)> = Vec::new();
        while let Ok(update) = rx.try_recv() {
            pending_updates.push(update);
        }

        let mut latest_valid_update: Option<(WsDepthUpdate, Instant)> = None;

        for (update, recv_time) in pending_updates {
            if update.final_update_id <= last_final_update_id {
                continue;
            }

            if !synced {
                if update.first_update_id <= snapshot_update_id
                    && update.final_update_id > snapshot_update_id
                {
                    latest_valid_update = Some((update, recv_time));
                }
                continue;
            }

            if update.prev_final_update_id == last_final_update_id
                || update.prev_final_update_id >= last_final_update_id
            {
                latest_valid_update = Some((update, recv_time));
            }
        }

        if let Some((update, recv_time)) = latest_valid_update {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            let event_time = update.event_time as i64;
            let network_latency = (now_ms - event_time - cached_clock_offset).max(0) as u64;
            stats_main.record(
                &stats_main.network_latency,
                &stats_main.network_count,
                network_latency,
            );

            if !synced
                && update.first_update_id <= snapshot_update_id
                && update.final_update_id > snapshot_update_id
            {
                let mut book = order_book_ws.write().await;
                book.apply_update(&update);
                last_final_update_id = update.final_update_id;
                synced = true;
            } else if synced {
                let lock_start = Instant::now();
                let mut book = order_book_ws.write().await;
                let lock_elapsed = lock_start.elapsed().as_micros() as u64;
                stats_main.record(
                    &stats_main.lock_acquire_time,
                    &stats_main.lock_acquire_count,
                    lock_elapsed,
                );

                let update_start = Instant::now();
                book.apply_update(&update);
                let update_elapsed = update_start.elapsed().as_micros() as u64;
                stats_main.record(
                    &stats_main.orderbook_update_time,
                    &stats_main.orderbook_update_count,
                    update_elapsed,
                );

                last_final_update_id = update.final_update_id;
            }

            let processing_elapsed = recv_time.elapsed().as_micros() as u64;
            stats_main.record(
                &stats_main.processing_latency,
                &stats_main.processing_count,
                processing_elapsed,
            );
        }

        let mut pending_trades: Vec<Trade> = Vec::new();
        while let Ok(trade) = trade_rx.try_recv() {
            pending_trades.push(trade);
        }

        if !pending_trades.is_empty() {
            let mut history = trade_history_ws.write().await;
            for trade in pending_trades {
                history.add_trade(trade);
            }
        }

        if last_render.elapsed() >= render_interval {
            let render_start = Instant::now();
            let book = order_book_render.read().await;

            cached_clock_offset = book.clock_offset;

            if let (Some((bid, _)), Some((ask, _))) = (book.best_bid(), book.best_ask()) {
                let mid_price = (bid + ask) / 2.0;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                let mut history = trade_history_render.write().await;
                history.add_price(now_ms, mid_price);
            }

            let history = trade_history_render.read().await;
            terminal.draw(|f| ui(f, &app, &book, &stats, &history))?;
            drop(book);
            drop(history);
            app.render_time_us = render_start.elapsed().as_micros() as u64;
            stats.record(&stats.render_time, &stats.render_count, app.render_time_us);
            last_render = Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    restore_terminal(&mut terminal)?;
    println!("\nGoodbye!");

    Ok(())
}
