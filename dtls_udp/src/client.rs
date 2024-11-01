use std::fs;
use std::io::{self, Write, Read};
use std::net::UdpSocket;
use udp_dtls::{Certificate, DtlsConnector, UdpChannel, SrtpProfile};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load root CA certificate
    let root_ca_data = fs::read("/users/dorlando/ons/certs/fullchain1.pem")?;
    let root_ca = Certificate::from_pem(&root_ca_data)?;
    // Set up the DTLS connector
    let connector = DtlsConnector::builder()
        .add_root_certificate(root_ca)
        .add_srtp_profile(SrtpProfile::Aes128CmSha180)
        .build()
        .expect("Failed to create connector");
    // Set up client socket
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("149.43.80.144:4444")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    println!("Connected to server at 149.43.80.144:4444");
    let client_channel = UdpChannel {
        socket: socket.try_clone().expect("Failed to clone socket"),
        remote_addr: socket.peer_addr()?,
    };
    let mut dtls_client = connector.connect("spock.cs.colgate.edu", client_channel).expect("Failed to connect DTLS");

    // Number of messages to send
    const NUM_MESSAGES: usize = 10000000;
    let mut total_rtt = 0;

    for i in 0..NUM_MESSAGES {
        let message = format!("Hello from client, message {}", i);
        let start_time = Instant::now();
        // Send the message to the server
        dtls_client.write_all(message.as_bytes()).expect("Failed to send message");
        println!("Sent: {}", message);
        // Receive the echoed message from the server
        let mut buf = [0u8; 1500];
        match dtls_client.read(&mut buf) {
            Ok(size) => {
                let end_time = Instant::now();
                let rtt = end_time.duration_since(start_time).as_micros();
                total_rtt += rtt;
                let received = String::from_utf8_lossy(&buf[..size]);
                println!("Received echo: '{}', RTT: {} µs", received, rtt);
            }
            Err(e) => {
                eprintln!("Failed to receive echo: {}", e);
            }
        }
    }

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
