use std::time::Instant;
use wtransport::{ClientConfig, Endpoint};
use anyhow::Result;

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

    const NUM_MESSAGES: usize = 10000000000;
    let mut total_rtt = 0;

    for i in 0..NUM_MESSAGES {
        let message = format!("Hello from client, message {}", i);
        let start_time = Instant::now();

        // Send datagram to server
        connection.send_datagram(message.as_bytes()).unwrap();
        println!("Sent: {}", message);

        // Wait for the echo response from the server
        match connection.receive_datagram().await {
            Ok(datagram) => {
                let payload = datagram.payload().to_vec();
                let received_data = String::from_utf8_lossy(&payload);
                let rtt = start_time.elapsed().as_micros();
                total_rtt += rtt;
                println!("Received echo: '{}', RTT: {} µs", received_data, rtt);
            }
            Err(e) => {
                println!("Error receiving datagram: {:?}", e);
                break;
            }
        }
    }

    // Calculate and print average RTT
    if NUM_MESSAGES > 0 {
        let average_rtt = total_rtt as f64 / NUM_MESSAGES as f64;
        println!("Total messages sent: {}", NUM_MESSAGES);
        println!("Total RTT: {} µs", total_rtt);
        println!("Average RTT: {:.2} µs", average_rtt);
    } else {
        println!("No messages sent.");
    }

    println!("Client done.");
    Ok(())
}
