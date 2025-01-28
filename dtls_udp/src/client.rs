use std::fs;
use std::io::{self, Write, Read};
use std::net::UdpSocket;
use udp_dtls::{Certificate, DtlsConnector, UdpChannel, SrtpProfile};
use std::time::{Duration, Instant};
use csv::Writer;
use serde_json::json;

fn save_measurements(rtt_samples: &[u128]) -> Result<(), Box<dyn std::error::Error>> {
    // Save raw measurements in CSV format
    let mut writer = Writer::from_path("udp_measurements.csv")?;
    writer.write_record(&["rtt"])?;

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

    let mut file = fs::File::create("udp_summary.json")?;
    file.write_all(serde_json::to_string_pretty(&summary)?.as_bytes())?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load root CA certificate
    //let root_ca_data = fs::read("/users/dorlando/ons/certs/cert1.pem")?;
    let root_ca_data = fs::read("/users/dorlando/ons/dtls_udp/signallite.io.pem")?;
    let root_ca = Certificate::from_pem(&root_ca_data)?;
    let connection_start = Instant::now();
    // Set up the DTLS connector
    let connector = DtlsConnector::builder()
        .add_root_certificate(root_ca)
        .add_srtp_profile(SrtpProfile::Aes128CmSha180)
        .build()
        .expect("Failed to create connector");

    // Set up client socket
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    //socket.connect("149.43.80.144:4444")?;
    socket.connect("204.48.31.168:4444")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    println!("Connected to server at port 4444");

    let client_channel = UdpChannel {
        socket: socket.try_clone().expect("Failed to clone socket"),
        remote_addr: socket.peer_addr()?,
    };

    let mut dtls_client = connector
        .connect("signallite.io", client_channel)
        .expect("Failed to connect DTLS");

    let connection_end = Instant::now();
    let connection_duration = connection_end.duration_since(connection_start);
    println!("Connection established in {} ms", connection_duration.as_millis());
    // Number of messages to send
    const NUM_MESSAGES: usize = 10000;
    let mut rtt_samples = Vec::new();

    for i in 0..NUM_MESSAGES {
        let message = format!("Hello from client, message {}", i);
        let start_time = Instant::now();

        // Send the message to the server
        dtls_client
            .write_all(message.as_bytes())
            .expect("Failed to send message");

        let mut buf = [0u8; 1500];
        match dtls_client.read(&mut buf) {
            Ok(size) => {
                let end_time = Instant::now();
                let rtt = end_time.duration_since(start_time).as_micros();
                rtt_samples.push(rtt);
                let received = String::from_utf8_lossy(&buf[..size]);
                println!("Received echo: '{}', RTT: {} µs", received, rtt);
            }
            Err(e) => {
                eprintln!("Failed to receive echo: {}", e);
            }
        }
    }

    if !rtt_samples.is_empty() {
        println!("Saving RTT data...");
        save_measurements(&rtt_samples).expect("Failed to save measurements");
        save_summary(&rtt_samples).expect("Failed to save summary");

        let total_rtt: u128 = rtt_samples.iter().sum();
        let average_rtt = total_rtt as f64 / rtt_samples.len() as f64;
        println!("Total messages sent: {}", rtt_samples.len());
        println!("Average RTT: {:.2} µs", average_rtt);
    } else {
        println!("No RTT data collected.");
    }

    println!("Client done.");
    Ok(())
}
