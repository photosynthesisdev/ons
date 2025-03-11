use std::{fs, io, path};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use anyhow::Context;
use clap::Parser;
use rustls::pki_types::CertificateDer;
use web_transport_quinn::Session;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "0.0.0.0:4433")]
    addr: std::net::SocketAddr,

    /// Use the certificates at this path, encoded as PEM.
    #[arg(long)]
    pub tls_cert: path::PathBuf,

    /// Use the private key at this path, encoded as PEM.
    #[arg(long)]
    pub tls_key: path::PathBuf,

    #[arg(long, default_value = "rtt_measurements.csv")]
    pub output_file: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::init_from_env(env);

    let args = Args::parse();

    // Read the PEM certificate chain
    let chain = fs::File::open(&args.tls_cert).context("failed to open cert file")?;
    let mut chain = io::BufReader::new(chain);

    let chain: Vec<CertificateDer> = rustls_pemfile::certs(&mut chain)
        .collect::<Result<_, _>>()
        .context("failed to load certs")?;

    anyhow::ensure!(!chain.is_empty(), "could not find certificate");

    // Read the PEM private key
    let keys = fs::File::open(&args.tls_key).context("failed to open key file")?;
    let key = rustls_pemfile::private_key(&mut io::BufReader::new(keys))
        .context("failed to load private key")?
        .context("missing private key")?;

    let mut server = web_transport_quinn::ServerBuilder::new()
        .with_addr(args.addr)
        .with_certificate(chain, key)?;

    log::info!("listening on {}", args.addr);

    while let Some(conn) = server.accept().await {
        let output_file = args.output_file.clone();
        tokio::spawn(async move {
            match run_conn(conn, output_file).await {
                Ok(_) => log::info!("connection completed"),
                Err(err) => log::error!("connection failed: {}", err),
            }
        });
    }

    Ok(())
}

async fn run_conn(request: web_transport_quinn::Request, output_file: String) -> anyhow::Result<()> {
    log::info!("received WebTransport request: {}", request.url());

    let session = request.ok().await.context("failed to accept session")?;
    log::info!("accepted session");

    if let Err(err) = run_session(session, output_file).await {
        log::error!("session error: {}", err);
    }

    Ok(())
}

async fn run_session(session: Session, output_file: String) -> anyhow::Result<()> {
    // Open CSV file for writing RTT measurements
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(output_file)
        .await?;

    // Write CSV header if file is empty
    if file.metadata().await?.len() == 0 {
        file.write_all(b"timestamp,rtt_ms\n").await?;
    }

    // Accept a single bidirectional stream for the session
    log::info!("waiting for bidirectional stream...");
    let (mut send, mut recv) = session.accept_bi().await?;
    log::info!("accepted stream");

    let mut buf = vec![0u8; 1024];
    
    // Wait for the first simulation message to synchronize tick timing
    log::info!("waiting for first tick message from client...");
    match recv.read(&mut buf).await? {
        Some(size) => {
            log::info!("received first tick message, starting tick loop");
            // Echo back the first message immediately
            let first_msg = &buf[..size];
            send.write_all(first_msg).await?;
            
            // If message is JSON, try to extract tick number for logging
            if let Ok(text) = std::str::from_utf8(first_msg) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
                    if let Some(tick) = json.get("tick") {
                        log::info!("first message is tick {}", tick);
                    }
                }
            }
            
            // Log the first measurement
            let timestamp = chrono::Utc::now().timestamp();
            let measurement = format!("{},0\n", timestamp);
            file.write_all(measurement.as_bytes()).await?;
        },
        None => {
            log::info!("client closed connection before sending first message");
            return Ok(());
        }
    };
    
    // Continue reading messages on the same stream
    while let Some(size) = recv.read(&mut buf).await? {
        let msg = &buf[..size];
        // Echo back the message
        send.write_all(msg).await?;
        
        // Calculate and log timing
        let timestamp = chrono::Utc::now().timestamp();
        let measurement = format!("{},0\n", timestamp); // RTT measurement moved to client
        file.write_all(measurement.as_bytes()).await?;
    }

    log::info!("Stream closed");
    Ok(())
}