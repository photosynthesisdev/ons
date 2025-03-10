use std::fs;
use std::io::{self, Write, Read};
use std::net::UdpSocket;
use udp_dtls::{Certificate, DtlsConnector, UdpChannel, SrtpProfile};
use std::time::{Duration, Instant};
use std::thread;
use std::collections::HashMap;
use csv::Writer;
use serde_json::json;

fn save_measurements(rtt_samples: &[u128]) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = Writer::from_path("udp_measurements.csv")?;
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

    let mut file = fs::File::create("udp_summary.json")?;
    file.write_all(serde_json::to_string_pretty(&summary)?.as_bytes())?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load the root CA certificate.
    let root_ca_data = fs::read("/users/dorlando/ons/dtls_udp/signallite.io.pem")?;
    //let root_ca_data = fs::read("/users/dorlando/ons/certs/fullchain1.pem")?;
    let root_ca = match Certificate::from_pem(&root_ca_data) {
        Ok(cert) => cert,
        Err(e) => {
            eprintln!("Certificate loading error: {:?}", e);
            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, 
                format!("Certificate loading failed: {:?}", e))));
        }
    };
    println!("Certificate loaded successfully");
    let connection_start = Instant::now();

    // Set up the DTLS connector.
    let connector = DtlsConnector::builder()
        .add_root_certificate(root_ca)
        .add_srtp_profile(SrtpProfile::Aes128CmSha180)
        // Uncomment these if they're available in the library
        // .danger_accept_invalid_certs(true) // Accept self-signed certs for testing
        // .danger_accept_invalid_hostnames(true) // Don't verify hostname for testing
        .build()
        .expect("Failed to create connector");
    
    println!("DTLS connector created successfully");

    // Set up the client socket in blocking mode for the handshake
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("204.48.31.168:4444")?;
    //socket.connect("149.43.80.144:4444")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    println!("Connecting to server at port 4444");

    let client_channel = UdpChannel {
        socket: socket.try_clone().expect("Failed to clone socket"),
        remote_addr: socket.peer_addr()?,
    };

    // Perform DTLS handshake with blocking socket
    let mut dtls_client = match connector.connect("signallite.io", client_channel) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("DTLS connection error: {:?}", e);
            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, 
                format!("DTLS connection failed: {:?}", e))));
        }
    };

    // Handshake succeeded, now set socket to non-blocking for our tick loop
    socket.set_nonblocking(true)?;

    let connection_end = Instant::now();
    println!("Connection established in {} ms", connection_end.duration_since(connection_start).as_millis());

    // Define tick rate and compute tick duration with microsecond precision.
    const TICK_RATE: u32 = 120;
    let tick_duration = Duration::from_micros(1_000_000 / TICK_RATE as u64);
    println!("Tick duration: {} µs", tick_duration.as_micros());

    // Set simulation duration to 3 minutes.
    let simulation_duration = Duration::from_secs(180);
    let simulation_start = Instant::now();
    let simulation_end = simulation_start + simulation_duration;

    let mut tick_count: u64 = 0;
    // Store the Instant at which each tick message is sent.
    let mut sent_timestamps: HashMap<u64, Instant> = HashMap::new();
    let mut rtt_samples: Vec<u128> = Vec::new();
    let mut buf = [0u8; 1500];

    // Run the simulation tick loop.
    while Instant::now() < simulation_end {
        let tick_start = Instant::now();

        // Process any incoming echoed messages (non-blocking).
        loop {
            match dtls_client.read(&mut buf) {
                Ok(size) if size > 0 => {
                    let received_str = String::from_utf8_lossy(&buf[..size]);
                    // Expecting JSON of the form: {"tick": <number>, "timestamp": <µs>}
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&received_str) {
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
                Ok(0) => break, // No more data available.
                Ok(_) => (), // This handles any other size (which shouldn't happen, but the compiler wants this case)
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("Error reading from DTLS connection: {}", e);
                    break;
                }
            }
        }

        // Prepare the tick message with the tick number and a precise timestamp (in µs from simulation start).
        let timestamp = Instant::now().duration_since(simulation_start).as_micros();
        let message = format!(r#"{{"tick":{},"timestamp":{}}}"#, tick_count, timestamp);
        sent_timestamps.insert(tick_count, Instant::now());

        // Send the tick message.
        match dtls_client.write_all(message.as_bytes()) {
            Ok(_) => println!("Sent tick {} at {} µs", tick_count, timestamp),
            Err(e) => {
                eprintln!("Error sending tick message: {:?}", e);
                break;
            }
        }
        tick_count += 1;

        // Sleep until the next tick boundary (adjusting for the time already spent processing).
        let elapsed = tick_start.elapsed();
        if elapsed < tick_duration {
            thread::sleep(tick_duration - elapsed);
        }
    }

    // After simulation, save and summarize the RTT data.
    if !rtt_samples.is_empty() {
        println!("Saving RTT data...");
        save_measurements(&rtt_samples).expect("Failed to save measurements");
        save_summary(&rtt_samples).expect("Failed to save summary");

        let total_rtt: u128 = rtt_samples.iter().sum();
        let average_rtt = total_rtt as f64 / rtt_samples.len() as f64;
        println!("Total ticks sent (with RTT measured): {}", rtt_samples.len());
        println!("Average RTT: {:.2} µs", average_rtt);
    } else {
        println!("No RTT data collected.");
    }

    println!("Client simulation complete after 3 minutes.");
    Ok(())
}
