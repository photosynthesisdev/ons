use futures_util::{StreamExt, SinkExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{client_async, tungstenite::protocol::Message};
use tokio_native_tls::native_tls::{TlsConnector as NativeTlsConnector};
use tokio_native_tls::TlsConnector;
use url::Url;
use std::time::{Instant, SystemTime};
use tokio::task;
use std::fs::File;
use std::io::{Write, Read};
use csv::Writer;
use serde_json::json;

fn save_measurements(rtt_samples: &[u128]) -> Result<(), Box<dyn std::error::Error>> {
    // Save raw measurements in CSV format
    let mut writer = Writer::from_path("websocket_measurements.csv")?;
    
    // Write headers
    writer.write_record(&["rtt"])?;
    
    // Write the RTT data
    for rtt in rtt_samples {
        writer.write_record(&[rtt.to_string()])?;
    }
    writer.flush()?;
    Ok(())
}

fn save_summary(rtt_samples: &[u128]) -> Result<(), Box<dyn std::error::Error>> {
    // Calculate statistics
    let mut sorted = rtt_samples.to_vec();
    sorted.sort_unstable();

    let avg = rtt_samples.iter().sum::<u128>() as f64 / rtt_samples.len() as f64;
    let p50 = sorted[rtt_samples.len() / 2];
    let p95 = sorted[(rtt_samples.len() as f64 * 0.95) as usize];
    let p99 = sorted[(rtt_samples.len() as f64 * 0.99) as usize];

    // Save summary to JSON
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

    let mut file = File::create("websocket_summary.json")?;
    file.write_all(serde_json::to_string_pretty(&summary)?.as_bytes())?;
    Ok(())
}

#[tokio::main]
async fn main() {
    //let url = Url::parse("wss://spock.cs.colgate.edu:4043").unwrap();
    let url = Url::parse("wss://signallite.io:4043").unwrap();
    println!("Connecting to {}", url);

    let mut cert_file = File::open("/users/dorlando/ons/websocket_rust/signallite_cert.pem").unwrap();
    //let mut cert_file = File::open("/users/dorlando/ons/certs/cert1.pem").unwrap();
    let mut cert = vec![];
    cert_file.read_to_end(&mut cert).unwrap();
    let mut builder = NativeTlsConnector::builder();
    builder.danger_accept_invalid_certs(true); // Disable cert validation (not for production!)
    builder.add_root_certificate(tokio_native_tls::native_tls::Certificate::from_pem(&cert).unwrap());
    let tls_connector = TlsConnector::from(builder.build().unwrap());

    let domain = url.host_str().expect("No host found in URL");
    let connection_start = Instant::now();

    let tcp_stream = TcpStream::connect(format!("{}:{}", domain, url.port_or_known_default().unwrap()))
        .await
        .expect("Failed to connect to TCP");
    let tls_stream = tls_connector
        .connect(domain, tcp_stream)
        .await
        .expect("Failed to perform TLS handshake");

    let (ws_stream, _) = client_async(url, tls_stream)
        .await
        .expect("Failed to establish WebSocket connection");

    let connection_end = Instant::now();

    // Calculate connection establishment time
    let connection_duration = connection_end.duration_since(connection_start);
    println!(
        "Connection established in {} ms",
        connection_duration.as_millis()
    );

    println!("Connected to the server");

    let (mut write, mut read) = ws_stream.split();
    let mut rtt_samples = Vec::new();
    let message_limit = 10000;

    for i in 0..message_limit {
        let timestamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_micros();
        let message = format!("Message {}|{}", i, timestamp);

        let start_time = Instant::now();
        if write.send(Message::Text(message)).await.is_err() {
            println!("Error sending message");
            break;
        }

        if let Some(Ok(response)) = read.next().await {
            if response.is_text() {
                let end_time = Instant::now();
                let rtt = end_time.duration_since(start_time).as_micros();
                rtt_samples.push(rtt);
            }
        }

        if i % 500 == 0 {
            task::yield_now().await; // Prevents task starvation
        }
    }

    if !rtt_samples.is_empty() {
        println!("Saving RTT data...");
        save_measurements(&rtt_samples).expect("Failed to save measurements");
        save_summary(&rtt_samples).expect("Failed to save summary");

        let total_rtt: u128 = rtt_samples.iter().sum();
        let average_rtt = total_rtt as f64 / rtt_samples.len() as f64;
        println!("Total messages sent: {}", rtt_samples.len());
        println!("Average RTT: {} µs", average_rtt);
    } else {
        println!("No RTT data collected.");
    }

    println!("Client disconnected.");
}
