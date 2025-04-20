use futures_util::{StreamExt, SinkExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{client_async, tungstenite::protocol::Message};
use tokio_native_tls::native_tls::{TlsConnector as NativeTlsConnector};
use tokio_native_tls::TlsConnector;
use url::Url;
use std::time::{Instant, Duration};
use std::collections::HashMap;
use tokio::time::interval;
use std::fs::File;
use std::io::{Write, Read};
use csv::Writer;
use serde_json::{json, Value};

// Define tick rate constants
const TICK_RATE: u32 = 128; // ticks per second
const TICK_DURATION_MICROS: u64 = 1_000_000 / TICK_RATE as u64;
const SIMULATION_DURATION_SECS: u64 = 180; // 3 minutes

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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    //let url = Url::parse("wss://spock.cs.colgate.edu:4043").unwrap();
    let url = Url::parse("wss://sculpter.dev:4043").unwrap();
    println!("Connecting to {}", url);
    let mut cert_file = File::open("/users/dorlando/ons/websocket_rust/sculpter_cert.pem").unwrap();
    //let mut cert_file = File::open("/users/dorlando/ons/websocket_rust/signallite_cert.pem").unwrap();
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

    // Create a tick interval
    let tick_duration = Duration::from_micros(TICK_DURATION_MICROS);
    println!("Tick duration: {} µs", TICK_DURATION_MICROS);
    let mut tick_interval = interval(tick_duration);

    // Split WebSocket stream for concurrent reading and writing
    let (mut write, mut read) = ws_stream.split();
    
    // Setup simulation state tracking
    let simulation_start = Instant::now();
    let simulation_end = simulation_start + Duration::from_secs(SIMULATION_DURATION_SECS);
    let mut tick_count: u64 = 0;
    let mut sent_timestamps: HashMap<u64, Instant> = HashMap::new();
    let mut rtt_samples: Vec<u128> = Vec::new();

    // Start the simulation tick loop
    println!("Starting tick-based simulation for {} seconds", SIMULATION_DURATION_SECS);
    
    while Instant::now() < simulation_end {
        // Wait for the next tick interval
        tick_interval.tick().await;
        
        // Prepare the tick message with the tick number and a precise timestamp
        let timestamp = simulation_start.elapsed().as_micros();
        let message = format!(r#"{{"tick":{},"timestamp":{}}}"#, tick_count, timestamp);
        sent_timestamps.insert(tick_count, Instant::now());
        
        // Send the tick message
        match write.send(Message::Text(message)).await {
            Ok(_) => println!("Sent tick {} at {} µs", tick_count, timestamp),
            Err(e) => {
                eprintln!("Error sending tick message: {:?}", e);
                break;
            }
        }
        tick_count += 1;
        
        // Process any incoming responses (using a timeout to avoid blocking)
        while let Ok(Some(message)) = tokio::time::timeout(
            Duration::from_millis(1), 
            read.next()
        ).await {
            match message {
                Ok(msg) if msg.is_text() => {
                    let received_str = msg.into_text()?;
                    // Parse the JSON message to extract the tick number
                    if let Ok(parsed) = serde_json::from_str::<Value>(&received_str) {
                        if let Some(tick_val) = parsed.get("tick") {
                            if let Some(tick) = tick_val.as_u64() {
                                if let Some(sent_time) = sent_timestamps.remove(&tick) {
                                    let rtt = Instant::now().duration_since(sent_time);
                                    rtt_samples.push(rtt.as_micros());
                                    println!("Tick {}: Received echo, RTT: {} µs", tick, rtt.as_micros());
                                }
                            }
                        }
                    }
                },
                Ok(_) => continue, // Ignore non-text messages
                Err(e) => {
                    eprintln!("Error reading from WebSocket: {:?}", e);
                    break;
                }
            }
        }
    }

    println!("Simulation complete after {} seconds", SIMULATION_DURATION_SECS);

    // After simulation, save and summarize the RTT data
    if !rtt_samples.is_empty() {
        println!("Saving RTT data...");
        save_measurements(&rtt_samples)?;
        save_summary(&rtt_samples)?;

        let total_rtt: u128 = rtt_samples.iter().sum();
        let average_rtt = total_rtt as f64 / rtt_samples.len() as f64;
        println!("Total ticks sent (with RTT measured): {}", rtt_samples.len());
        println!("Average RTT: {:.2} µs", average_rtt);
    } else {
        println!("No RTT data collected.");
    }

    println!("Client disconnected.");
    Ok(())
}
