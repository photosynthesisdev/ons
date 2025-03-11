use std::{fs, io, path, time::{Instant, Duration}, collections::HashMap};
use anyhow::Context;
use clap::Parser;
use rustls::pki_types::CertificateDer;
use url::Url;
use tokio::time::sleep;
use serde_json::json;
use csv::Writer;
use std::io::Write;
use web_transport_quinn;
use rustls;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "https://signallite.io:4433")]
    url: Url,

    /// Accept the certificates at this path, encoded as PEM.
    #[arg(long)]
    pub tls_cert: path::PathBuf,

    #[arg(long, default_value = "180")]
    simulation_duration_secs: u64,
}

fn save_measurements(rtt_samples: &[u128]) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = Writer::from_path("webtransport_measurements.csv")?;
    writer.write_record(&["rtt"])?;
    
    for rtt in rtt_samples {
        writer.write_record(&[rtt.to_string()])?;
    }
    writer.flush()?;
    Ok(())
}

fn save_summary(rtt_samples: &[u128]) -> Result<(), Box<dyn std::error::Error>> {
    let mut sorted = rtt_samples.to_vec();
    sorted.sort_unstable();

    let avg = rtt_samples.iter().sum::<u128>() as f64 / rtt_samples.len() as f64;
    let p50 = sorted[rtt_samples.len() / 2];
    let p95 = sorted[(rtt_samples.len() as f64 * 0.95) as usize];
    let p99 = sorted[(rtt_samples.len() as f64 * 0.99) as usize];

    let summary = json!({
        "sample_count": rtt_samples.len(),
        "metrics": {
            "rtt": {
                "avg": avg,
                "p50": p50,
                "p95": p95,
                "p99": p99
            }
        }
    });

    let mut file = fs::File::create("webtransport_summary.json")?;
    file.write_all(serde_json::to_string_pretty(&summary)?.as_bytes())?;
    Ok(())
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

    // Create a new client with default settings
    let client = web_transport_quinn::ClientBuilder::new().with_server_certificates(chain)?;
    log::info!("connecting to {}", args.url);
    
    let connection_start = Instant::now();
    let session = client.connect(&args.url).await?;
    let connection_duration = connection_start.elapsed();
    log::info!("connected in {} ms", connection_duration.as_millis());

    // Define tick rate and compute tick duration with microsecond precision
    const TICK_RATE: u32 = 120; // 120 ticks per second
    let tick_duration = Duration::from_micros(1_000_000 / TICK_RATE as u64);
    log::info!("Tick duration: {} µs", tick_duration.as_micros());

    // Set simulation duration
    let simulation_duration = Duration::from_secs(args.simulation_duration_secs);
    let simulation_start = Instant::now();
    let simulation_end = simulation_start + simulation_duration;

    let mut tick_count: u64 = 0;
    let mut sent_timestamps: HashMap<u64, Instant> = HashMap::new();
    let mut rtt_samples: Vec<u128> = Vec::new();

    // Open a single bidirectional stream for all messages
    let (mut send, mut recv) = session.open_bi().await?;
    let mut buf = vec![0u8; 1024];
    
    // Run the simulation tick loop
    while Instant::now() < simulation_end {
        let tick_start = Instant::now();
        
        // Process any incoming echoed messages (non-blocking)
        loop {
            match tokio::time::timeout(Duration::from_millis(1), recv.read(&mut buf)).await {
                Ok(Ok(Some(size))) => {
                    if let Ok(received_str) = std::str::from_utf8(&buf[..size]) {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(received_str) {
                            if let Some(tick_val) = parsed.get("tick") {
                                if let Some(tick) = tick_val.as_u64() {
                                    if let Some(sent_time) = sent_timestamps.remove(&tick) {
                                        let rtt = Instant::now().duration_since(sent_time);
                                        rtt_samples.push(rtt.as_micros());
                                        log::info!("Tick {}: Received echo, RTT: {} µs", tick, rtt.as_micros());
                                    }
                                }
                            }
                        }
                    }
                },
                // Break on timeout or no more data
                Ok(Ok(None)) => break,
                Ok(Err(e)) => {
                    log::error!("Error reading from stream: {:?}", e);
                    break;
                },
                Err(_) => break, // Timeout occurred
            }
        }
        
        // Prepare the tick message with the tick number and precise timestamp
        let timestamp = Instant::now().duration_since(simulation_start).as_micros();
        let message = format!(r#"{{"tick":{},"timestamp":{}}}"#, tick_count, timestamp);
        sent_timestamps.insert(tick_count, Instant::now());
        
        // Send the tick message
        match send.write_all(message.as_bytes()).await {
            Ok(_) => log::info!("Sent tick {} at {} µs", tick_count, timestamp),
            Err(e) => {
                log::error!("Error sending tick message: {:?}", e);
                break;
            }
        }
        tick_count += 1;
        
        // Sleep until the next tick boundary
        let elapsed = tick_start.elapsed();
        if elapsed < tick_duration {
            sleep(tick_duration - elapsed).await;
        }
    }

    // Close the stream after all messages are sent
    send.finish()?;

    if !rtt_samples.is_empty() {
        log::info!("Saving RTT data...");
        save_measurements(&rtt_samples).expect("Failed to save measurements");
        save_summary(&rtt_samples).expect("Failed to save summary");

        let average_rtt = rtt_samples.iter().sum::<u128>() as f64 / rtt_samples.len() as f64;
        log::info!("Total ticks sent (with RTT measured): {}", rtt_samples.len());
        log::info!("Average RTT: {:.2} µs", average_rtt);
    } else {
        log::info!("No RTT data collected.");
    }

    log::info!("Client simulation complete after {} seconds.", args.simulation_duration_secs);
    Ok(())
}