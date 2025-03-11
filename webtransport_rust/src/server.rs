use std::{fs, io, path, time::{Duration, Instant}};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use anyhow::Context;
use clap::Parser;
use rustls::pki_types::CertificateDer;
use web_transport_quinn::Session;
use tokio::time::interval;
use tokio::sync::{mpsc, Mutex};
use std::sync::Arc;
use serde_json::Value;
use bytes::Bytes;

// Define tick rate constants
const TICK_RATE: u32 = 30; // ticks per second, matching client
const TICK_DURATION_MICROS: u64 = 1_000_000 / TICK_RATE as u64;
const MAX_DATAGRAMS_PER_TICK: usize = 10; // Limit datagrams processed per tick to avoid overload

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
    
    /// Use datagram extension instead of streams
    #[arg(long)]
    use_datagrams: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::init_from_env(env);

    let args = Args::parse();

    log::info!("Using tick rate of {} ticks per second ({}µs per tick)", TICK_RATE, TICK_DURATION_MICROS);

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
    
    if args.use_datagrams {
        log::info!("Server configured to use WebTransport datagrams");
    } else {
        log::info!("Server configured to use WebTransport bidirectional streams");
    }

    while let Some(conn) = server.accept().await {
        let output_file = args.output_file.clone();
        let use_datagrams = args.use_datagrams;
        tokio::spawn(async move {
            match run_conn(conn, output_file, use_datagrams).await {
                Ok(_) => log::info!("connection completed"),
                Err(err) => log::error!("connection failed: {}", err),
            }
        });
    }

    Ok(())
}

async fn run_conn(request: web_transport_quinn::Request, output_file: String, use_datagrams: bool) -> anyhow::Result<()> {
    log::info!("received WebTransport request: {}", request.url());

    let session = request.ok().await.context("failed to accept session")?;
    log::info!("accepted session");

    if let Err(err) = run_session(session, output_file, use_datagrams).await {
        log::error!("session error: {}", err);
    }

    Ok(())
}

async fn run_session(session: Session, output_file: String, use_datagrams: bool) -> anyhow::Result<()> {
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
    
    if use_datagrams {
        // Using datagram extension
        log::info!("Starting datagram-based session...");
        
        // Message files are shared between tasks
        let file = Arc::new(Mutex::new(file));
        
        // The session object needs to be shared among tasks
        let session = Arc::new(session);
        let session_for_receiver = session.clone();
        let session_for_first = session.clone();
        
        // Create a channel for processing received datagrams
        let (tx, mut rx) = mpsc::channel::<Bytes>(100);
        
        // Create a queue for processing datagrams on the tick schedule
        let datagram_queue = Arc::new(Mutex::new(Vec::<Bytes>::new()));
        let datagram_queue_clone = datagram_queue.clone();
        
        // Datagram receiver task - continuously read datagrams and forward them to processing
        let receiver_task = tokio::spawn(async move {
            loop {
                match session_for_receiver.read_datagram().await {
                    Ok(datagram) => {
                        // Log the message if it contains a valid tick value
                        if let Ok(text) = std::str::from_utf8(&datagram) {
                            if let Ok(json) = serde_json::from_str::<Value>(text) {
                                if let Some(tick) = json.get("tick") {
                                    log::info!("Received tick {} datagram", tick);
                                }
                            }
                        }
                        
                        // Forward the datagram to the processing queue
                        if let Err(e) = tx.send(datagram).await {
                            log::error!("Failed to forward datagram: {}", e);
                            break;
                        }
                    },
                    Err(e) => {
                        log::error!("Error reading datagram: {:?}", e);
                        break;
                    }
                }
            }
            log::info!("Datagram receiver task completed");
        });
        
        // Datagram queue task - collect datagrams from channel to be processed by tick
        let queue_task = tokio::spawn(async move {
            while let Some(datagram) = rx.recv().await {
                let mut queue = datagram_queue_clone.lock().await;
                queue.push(datagram);
            }
            log::info!("Datagram queue task completed");
        });
        
        // The first received datagram might be important for synchronization
        // Wait for at least one datagram before starting the tick processing
        log::info!("Waiting for first datagram from client...");
        
        let first_datagram = match tokio::time::timeout(Duration::from_secs(30), session_for_first.read_datagram()).await {
            Ok(Ok(datagram)) => {
                log::info!("Received first datagram, starting echo processing");
                // Try to extract and log tick information
                if let Ok(text) = std::str::from_utf8(&datagram) {
                    if let Ok(json) = serde_json::from_str::<Value>(text) {
                        if let Some(tick) = json.get("tick") {
                            log::info!("First datagram is tick {}", tick);
                        }
                    }
                }
                Some(datagram)
            },
            Ok(Err(e)) => {
                log::error!("Error receiving first datagram: {:?}", e);
                None
            },
            Err(_) => {
                log::warn!("Timeout waiting for first datagram");
                None
            }
        };
        
        // We need another session clone for the tick task
        let session_for_tick = session.clone();
        
        // Echo the first datagram immediately if we got one
        if let Some(datagram) = first_datagram {
            if let Err(e) = session.send_datagram(datagram.clone()) {
                log::error!("Error echoing first datagram: {:?}", e);
            } else {
                log::info!("Echoed first datagram");
                
                // Log the first measurement
                let timestamp = chrono::Utc::now().timestamp();
                let measurement = format!("{},0\n", timestamp);
                let mut file_guard = file.lock().await;
                file_guard.write_all(measurement.as_bytes()).await?;
            }
        }
        
        // Main tick processing task
        let tick_task = tokio::spawn(async move {
            let tick_duration = Duration::from_micros(TICK_DURATION_MICROS);
            let mut tick_interval = interval(tick_duration);
            
            loop {
                // Wait for the next tick
                tick_interval.tick().await;
                let tick_start = Instant::now();
                
                // Process all datagrams in the queue (up to MAX_DATAGRAMS_PER_TICK)
                let datagrams_to_process = {
                    let mut queue = datagram_queue.lock().await;
                    let count = queue.len().min(MAX_DATAGRAMS_PER_TICK);
                    if count == 0 {
                        vec![] // No datagrams to process
                    } else {
                        // Take only the first MAX_DATAGRAMS_PER_TICK datagrams
                        let datagrams = queue[0..count].to_vec();
                        // Remove the processed datagrams
                        queue.drain(0..count);
                        datagrams
                    }
                };
                
                // Echo each datagram back
                for datagram in datagrams_to_process {
                    match session_for_tick.send_datagram(datagram.clone()) {
                        Ok(_) => {
                            // Try to log the tick number if it's a JSON datagram
                            if let Ok(text) = std::str::from_utf8(&datagram) {
                                if let Ok(json) = serde_json::from_str::<Value>(text) {
                                    if let Some(tick) = json.get("tick") {
                                        log::info!("Echoed tick {} datagram", tick);
                                    }
                                }
                            }
                            
                            // Log the timestamp for this tick
                            let timestamp = chrono::Utc::now().timestamp();
                            let measurement = format!("{},0\n", timestamp); // RTT measured on client
                            
                            // Write to file with mutex protection
                            let mut file_guard = file.lock().await;
                            if let Err(e) = file_guard.write_all(measurement.as_bytes()).await {
                                log::error!("Error writing to file: {:?}", e);
                            }
                        },
                        Err(e) => {
                            log::error!("Error echoing datagram: {:?}", e);
                            // Continue anyway since datagrams are unreliable
                        }
                    }
                }
                
                // Log time spent in this tick for debugging
                let elapsed = tick_start.elapsed();
                if elapsed > Duration::from_micros(TICK_DURATION_MICROS) {
                    log::warn!("Tick processing took {}µs, exceeding tick duration of {}µs", 
                               elapsed.as_micros(), TICK_DURATION_MICROS);
                }
            }
        });
        
        // Wait for any task to complete (which means the connection is closing)
        tokio::select! {
            _ = receiver_task => log::info!("Datagram receiver task completed"),
            _ = queue_task => log::info!("Datagram queue task completed"),
            _ = tick_task => log::info!("Tick task completed"),
        }
        
        log::info!("Datagram session ended");
    } else {
        // Using bidirectional streams (original implementation)
        log::info!("Starting stream-based session...");
        
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
        
        // Set up message processing channel
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(100);
        
        // Shared message queue for tick processing
        let message_queue = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let message_queue_clone = message_queue.clone();
        
        // Receiver task: process incoming messages and add them to the queue
        let receiver_task = tokio::spawn(async move {
            let mut buffer = vec![0u8; 1024];
            
            while let Some(size) = match recv.read(&mut buffer).await {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Error reading from stream: {:?}", e);
                    None
                }
            } {
                let message = buffer[..size].to_vec();
                
                // Log the message if it's valid JSON with a tick number
                if let Ok(text) = std::str::from_utf8(&message) {
                    if let Ok(json) = serde_json::from_str::<Value>(text) {
                        if let Some(tick) = json.get("tick") {
                            log::info!("Received tick {} message", tick);
                        }
                    }
                }
                
                // Queue the message for processing in the next tick
                if let Err(e) = tx.send(message).await {
                    log::error!("Failed to send message to processing queue: {}", e);
                    break;
                }
            }
            
            log::info!("Client disconnected or stream closed");
        });
        
        // Message processor: collect messages from the channel into the queue
        let processor_task = tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                let mut queue = message_queue_clone.lock().await;
                queue.push(message);
            }
        });
        
        // Tick task: process messages on each tick
        let tick_task = tokio::spawn(async move {
            let tick_duration = Duration::from_micros(TICK_DURATION_MICROS);
            let mut tick_interval = interval(tick_duration);
            let file = Arc::new(Mutex::new(file));
            
            loop {
                // Wait for the next tick
                tick_interval.tick().await;
                let tick_start = Instant::now();
                
                // Process all messages in the queue
                let messages_to_process = {
                    let mut queue = message_queue.lock().await;
                    let messages = queue.clone();
                    queue.clear();
                    messages
                };
                
                // Echo each message back
                for message in messages_to_process {
                    match send.write_all(&message).await {
                        Ok(_) => {
                            // Try to log the tick number if it's a JSON message
                            if let Ok(text) = std::str::from_utf8(&message) {
                                if let Ok(json) = serde_json::from_str::<Value>(text) {
                                    if let Some(tick) = json.get("tick") {
                                        log::info!("Echoed tick {} message", tick);
                                    }
                                }
                            }
                            
                            // Log the timestamp for this tick
                            let timestamp = chrono::Utc::now().timestamp();
                            let measurement = format!("{},0\n", timestamp); // RTT measured on client
                            
                            // Write to file with mutex protection
                            let mut file_guard = file.lock().await;
                            if let Err(e) = file_guard.write_all(measurement.as_bytes()).await {
                                log::error!("Error writing to file: {:?}", e);
                            }
                        },
                        Err(e) => {
                            log::error!("Error sending message: {}", e);
                            return;
                        }
                    }
                }
                
                // Log time spent in this tick for debugging
                let elapsed = tick_start.elapsed();
                if elapsed > Duration::from_micros(TICK_DURATION_MICROS) {
                    log::warn!("Tick processing took {}µs, exceeding tick duration of {}µs", 
                            elapsed.as_micros(), TICK_DURATION_MICROS);
                }
            }
        });
        
        // Wait for any task to complete (which means the connection is closing)
        tokio::select! {
            _ = receiver_task => log::info!("Receiver task completed"),
            _ = processor_task => log::info!("Processor task completed"),
            _ = tick_task => log::info!("Tick task completed"),
        }

        log::info!("Stream closed");
    }
    
    Ok(())
}