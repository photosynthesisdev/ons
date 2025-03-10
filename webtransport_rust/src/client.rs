use std::{fs, io, path, time::{Instant, SystemTime}};
use anyhow::Context;
use clap::Parser;
use rustls::pki_types::CertificateDer;
use url::Url;
use tokio::task;
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

    #[arg(long, default_value = "10000")]
    message_count: usize,
}

fn save_measurements(rtt_samples: &[f64]) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = Writer::from_path("webtransport_measurements.csv")?;
    writer.write_record(&["rtt"])?;
    
    for rtt in rtt_samples {
        writer.write_record(&[format!("{:.3}", rtt)])?;
    }
    writer.flush()?;
    Ok(())
}

fn save_summary(rtt_samples: &[f64]) -> Result<(), Box<dyn std::error::Error>> {
    let mut sorted = rtt_samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let avg = rtt_samples.iter().sum::<f64>() / rtt_samples.len() as f64;
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

    let mut rtt_samples: Vec<f64> = Vec::new();

    // Open a single bidirectional stream for all messages
    let (mut send, mut recv) = session.open_bi().await?;
    
    for i in 0..args.message_count {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let message = format!("Message {}|{}", i, timestamp);

        let start_time = Instant::now();
        
        send.write_all(message.as_bytes()).await?;
        
        let mut buf = vec![0u8; 1024];
        if let Some(size) = recv.read(&mut buf).await? {
            let rtt = start_time.elapsed().as_micros() as f64;
            rtt_samples.push(rtt);
        }

        if i % 500 == 0 {
            log::info!("Sent {} messages", i);
            task::yield_now().await;
        }
    }

    // Close the stream after all messages are sent
    send.finish()?;

    if !rtt_samples.is_empty() {
        log::info!("Saving RTT data...");
        save_measurements(&rtt_samples).expect("Failed to save measurements");
        save_summary(&rtt_samples).expect("Failed to save summary");

        let average_rtt = rtt_samples.iter().sum::<f64>() / rtt_samples.len() as f64;
        log::info!("Total messages sent: {}", rtt_samples.len());
        log::info!("Average RTT: {:.3} ms", average_rtt);
    } else {
        log::info!("No RTT data collected.");
    }

    log::info!("Client disconnected.");
    Ok(())
}