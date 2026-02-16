mod channel;
mod server;

use clap::Parser;
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(name = "agent-dash-relay", about = "Encrypted WebSocket relay for agent-dash")]
struct Cli {
    /// Address to bind to
    #[arg(long, default_value = "0.0.0.0:8443")]
    bind: String,

    /// Maximum buffered messages per channel
    #[arg(long, default_value = "1000")]
    max_buffer: usize,

    /// Channel TTL in seconds (evict idle channels with no peers)
    #[arg(long, default_value = "86400")]
    channel_ttl: u64,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let listener = TcpListener::bind(&cli.bind).await.unwrap_or_else(|e| {
        eprintln!("Failed to bind to {}: {e}", cli.bind);
        std::process::exit(1);
    });

    println!("agent-dash-relay listening on {}", cli.bind);

    let channel_mgr = channel::spawn(cli.max_buffer, cli.channel_ttl);

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("Accept error: {e}");
                continue;
            }
        };

        let mgr = channel_mgr.clone();
        tokio::spawn(server::handle_connection(stream, addr, mgr));
    }
}
