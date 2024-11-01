use std::io::{self, Read, Write};
use std::net::UdpSocket;
use udp_dtls::{Certificate, DtlsAcceptor, Identity, UdpChannel};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load PKCS#12 identity from the generated `identity.p12` file
    let pkcs12_data = fs::read("identity.p12")?;
    let identity = Identity::from_pkcs12(&pkcs12_data, "")?;

    // Create the DTLS acceptor
    let acceptor = DtlsAcceptor::builder(identity).build().expect("Failed to create acceptor");

    let socket = UdpSocket::bind("0.0.0.0:4444")?;
    println!("Server listening on 0.0.0.0:4444");

    loop {
        // Initial receive to capture client address for DTLS setup
        let mut buf = [0u8; 1500];
        let (size, addr) = socket.recv_from(&mut buf)?;

        // Set up the DTLS channel using the client's address
        let server_channel = UdpChannel {
            socket: socket.try_clone().expect("Failed to clone socket"),
            remote_addr: addr,
        };

        // Accept the DTLS connection and perform the handshake
        let mut dtls_server = acceptor.accept(server_channel).expect("Failed to accept DTLS connection");
        println!("DTLS handshake completed with client {}", addr);

        // Loop to continuously read and echo messages for this client
        loop {
            // Buffer for reading message
            let mut message = [0u8; 1500];
            match dtls_server.read(&mut message) {
                Ok(size) => {
                    let received_msg = String::from_utf8_lossy(&message[..size]);
                    println!("Received from client {}: {:?}", addr, received_msg);

                    // Echo the message back to the client
                    dtls_server.write_all(&message[..size]).expect("Failed to send message");
                    println!("Echoed message back to client {}", addr);
                }
                Err(e) => {
                    eprintln!("Error reading from client {}: {:?}", addr, e);
                    break; // Exit the loop on read error
                }
            }
        }
    }
}
