use std::io::{self, Read, Write};
use std::net::UdpSocket;
use udp_dtls::{DtlsAcceptor, Identity, UdpChannel}; 
use std::fs;
use std::time::{Duration, Instant};
use std::thread;

const TICK_RATE: u32 = 30; // ticks per second; change as needed
const TICK_DURATION: Duration = Duration::from_micros(1_000_000u64 / TICK_RATE as u64); 

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load PKCS#12 identity from the generated `identity.p12` file
    let pkcs12_data = fs::read("identity_backup.p12")?;
    let identity = Identity::from_pkcs12(&pkcs12_data, "")?;

    // Create the DTLS acceptor
    let acceptor = DtlsAcceptor::builder(identity).build().expect("Failed to create acceptor");

    // Bind to UDP port 4444
    let socket = UdpSocket::bind("0.0.0.0:4444")?;
    // Keep socket in blocking mode during handshake phase
    println!("Server listening on 0.0.0.0:4444");

    loop {
        // Wait for an initial packet from a client to learn its address for DTLS setup.
        let mut buf = [0u8; 1500];
        
        // Handle incoming connection
        let (_, addr) = socket.recv_from(&mut buf)?;
        println!("Received initial packet from {}", addr);
        
        // Set up the DTLS channel using the client's address.
        let server_channel = UdpChannel {
            socket: socket.try_clone().expect("Failed to clone socket"),
            remote_addr: addr,
        };

        // Accept the DTLS connection and perform the handshake.
        let mut dtls_server = match acceptor.accept(server_channel) {
            Ok(server) => server,
            Err(e) => {
                eprintln!("DTLS accept error: {:?}", e);
                continue; // Skip this connection attempt and wait for another one
            }
        };
        println!("DTLS handshake completed with client {}", addr);
        
        // Now that handshake is complete, set to non-blocking mode for the tick loop
        socket.set_nonblocking(true)?;
        
        // Wait for the first simulation message to synchronize tick timing.
        let first_message = loop {
            let mut message = [0u8; 1500];
            match dtls_server.read(&mut message) {
                Ok(size) if size > 0 => break message[..size].to_vec(),
                Ok(_) => continue,
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // No message yet; sleep briefly and retry.
                    thread::sleep(Duration::from_millis(1));
                    continue;
                },
                Err(e) => {
                    eprintln!("Error reading first simulation message: {:?}", e);
                    continue;
                }
            }
        };

        println!("Received first simulation message from client, starting tick loop");

        // Immediately echo the first simulation message.
        dtls_server.write_all(&first_message)?;

        // Start the tick loop.
        loop {
            let tick_start = Instant::now();

            // Process all available incoming messages during this tick.
            loop {
                let mut message = [0u8; 1500];
                match dtls_server.read(&mut message) {
                    Ok(size) if size > 0 => {
                        let received_str = String::from_utf8_lossy(&message[..size]);
                        println!("Tick processing: received from {}: {}", addr, received_str);
                        // Echo the message back to the client.
                        dtls_server.write_all(&message[..size])?;
                        println!("Tick processing: echoed message to {}", addr);
                    },
                    Ok(_) => break, // No data was read.
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        eprintln!("Error reading from client {}: {:?}", addr, e);
                        break;
                    }
                }
            }

            // Sleep until the next tick boundary (accounting for processing time).
            let elapsed = tick_start.elapsed();
            if elapsed < TICK_DURATION {
                thread::sleep(TICK_DURATION - elapsed);
            }
        }
    }
}
