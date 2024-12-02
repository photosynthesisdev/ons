use std::time::Instant;
use wtransport::{ClientConfig, Endpoint};
use anyhow::Result;
use csv::Writer;

fn save_measurements(rtt_samples: &[u128]) -> Result<(), Box<dyn std::error::Error>> {
    // Save raw RTT measurements to a CSV file
    let mut writer = Writer::from_path("webtransport_measurements.csv")?;
    writer.write_record(&["rtt"])?;

    for rtt in rtt_samples {
        writer.write_record(&[rtt.to_string()])?;
    }

    writer.flush()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();

    let connection = Endpoint::client(config)
        .unwrap()
        .connect("https://spock.cs.colgate.edu:4433")
        .await
        .unwrap();

    const NUM_MESSAGES: usize = 100000;
    let mut rtt_samples = Vec::new();

    for i in 0..NUM_MESSAGES {
        let message = format!("Hello from client, message {}", i);
        let start_time = Instant::now();

        // Send datagram to server
        connection.send_datagram(message.as_bytes()).unwrap();
        println!("Sent: {}", message);

        // Wait for the echo response from the server
        match connection.receive_datagram().await {
            Ok(_) => {
                let rtt = start_time.elapsed().as_micros();
                rtt_samples.push(rtt);
            }
            Err(e) => {
                println!("Error receiving datagram: {:?}", e);
                break;
            }
        }
    }

    // Save RTT measurements to a CSV file
    if !rtt_samples.is_empty() {
        println!("Saving RTT data...");
        save_measurements(&rtt_samples).expect("Failed to save measurements");

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
