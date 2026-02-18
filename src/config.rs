use std::io::{stdin, BufRead, Write};

pub fn get_user_config() -> (String, f64) {
    println!("\x1b[1;36m╔═══════════════════════════════════════════════════════════════╗\x1b[0m");
    println!("\x1b[1;36m║                       ORDER BOOK VIEWER                       ║\x1b[0m");
    println!("\x1b[1;36m╚═══════════════════════════════════════════════════════════════╝\x1b[0m");
    println!();

    println!("\x1b[1;33mEnter trading symbol (e.g., btcusdt, ethusdt, solusdt):\x1b[0m");
    print!("\x1b[97m> \x1b[0m");
    std::io::stdout().flush().ok();

    let stdin = stdin();
    let mut symbol = String::new();
    stdin.lock().read_line(&mut symbol).ok();
    let symbol = symbol.trim().to_lowercase();
    let symbol = if symbol.is_empty() {
        "btcusdt".to_string()
    } else {
        symbol
    };

    println!();
    println!("\x1b[1;33mEnter price bin width (e.g., 0.001, 1, 10, 100):\x1b[0m");
    print!("\x1b[97m> \x1b[0m");
    std::io::stdout().flush().ok();

    let mut bin_input = String::new();
    stdin.lock().read_line(&mut bin_input).ok();
    let bucket_size: f64 = bin_input.trim().parse().unwrap_or(1.0);
    let bucket_size = if bucket_size <= 0.0 { 1.0 } else { bucket_size };

    println!();
    println!("\x1b[32m✓ Symbol: {}\x1b[0m", symbol.to_uppercase());
    println!("\x1b[32m✓ Bin width: {}\x1b[0m", bucket_size);
    println!();

    (symbol, bucket_size)
}
