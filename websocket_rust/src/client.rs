use futures_util::{StreamExt, SinkExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{client_async, tungstenite::protocol::Message};
use tokio_native_tls::native_tls::{TlsConnector as NativeTlsConnector};
use tokio_native_tls::TlsConnector;
use url::Url;
use std::time::{Instant, SystemTime};
use tokio::task;
use std::fs::File;
use std::io::Read;

#[tokio::main]
async fn main() {
    let url = Url::parse("wss://spock.cs.colgate.edu:4043").unwrap();
    println!("Connecting to {}", url);
    let mut cert_file = File::open("/users/dorlando/ons/certs/fullchain1.pem").unwrap();
    let mut cert = vec![];
    cert_file.read_to_end(&mut cert).unwrap();
    let mut builder = NativeTlsConnector::builder();
    // Disables certificate validation -- we would never do this in production. 
    builder.danger_accept_invalid_certs(true);
    builder.add_root_certificate(tokio_native_tls::native_tls::Certificate::from_pem(&cert).unwrap());
    let tls_connector = builder.build().unwrap();
    let tls_connector = TlsConnector::from(tls_connector);
    // Establish a TCP connection first
    let domain = url.host_str().expect("No host found in URL");
    let tcp_stream = TcpStream::connect(format!("{}:{}", domain, url.port_or_known_default().unwrap()))
        .await
        .expect("Failed to connect to TCP");
    // Perform the TLS handshake over the TCP connection
    let tls_stream = tls_connector
        .connect(domain, tcp_stream)
        .await
        .expect("Failed to perform TLS handshake");
    // Upgrade the TLS stream to a WebSocket connection
    let (ws_stream, _) = client_async(url, tls_stream)
        .await
        .expect("Failed to establish WebSocket connection");
    println!("Connected to the server");
    let (mut write, mut read) = ws_stream.split();
    let mut rtt_samples = Vec::new();
    let message_limit = 100000;
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
        task::yield_now().await;
    }
    if !rtt_samples.is_empty() {
        let total_rtt: u128 = rtt_samples.iter().sum();
        let average_rtt = total_rtt as f64 / rtt_samples.len() as f64;
        println!("Total messages sent: {}", rtt_samples.len());
        println!("Average RTT: {} µs", average_rtt);
    } else {
        println!("No RTT data collected.");
    }
    println!("Client disconnected.");
}
