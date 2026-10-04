use std::sync::Arc;

use clap::Parser;
use tokio::sync::{mpsc, Mutex};
use tracing_subscriber::EnvFilter;

use mush_core::transport::{self, Node};

/// Mush A'-CLI node (MUSH-A'-CLI-SPEC).
#[derive(Parser, Debug)]
#[command(name = "mush-node", version, about = "Mush A' prototype node (Rust CLI)")]
struct Args {
    /// Display name of this node.
    #[arg(long, default_value = "anon")]
    name: String,

    /// TCP port to listen on.
    #[arg(long, default_value_t = 7878)]
    port: u16,

    /// Invite link to join at startup (mush://...).
    #[arg(long)]
    join: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    let args = Args::parse();
    let node = Arc::new(Mutex::new(Node::new(&args.name, args.port)));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<String>();

    // Startup join (--join mush://...).
    if let Some(invite) = args.join {
        let tx = cmd_tx.clone();
        let _ = tx.send(format!("/join {invite}"));
    }

    // Stdin → command channel.
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if cmd_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    if let Err(e) = transport::run(node, cmd_rx, None).await {
        eprintln!("[FATAL] node failed: {e}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
