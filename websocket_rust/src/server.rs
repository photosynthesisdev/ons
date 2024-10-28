use futures_util::{StreamExt, SinkExt};
use tokio::net::TcpListener;
use tokio_native_tls::TlsAcceptor;
use tokio_native_tls::native_tls::{Identity, TlsAcceptor as NativeTlsAcceptor};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let addr = "0.0.0.0:4043".to_string();
    let mut cert_file = File::open("/users/dorlando/ons/certs/cert1.pem").unwrap();
    let mut cert = vec![];
    cert_file.read_to_end(&mut cert).unwrap();
    let mut key_file = File::open("/users/dorlando/ons/certs/privkey1.pem").unwrap();
    let mut key = vec![];
    key_file.read_to_end(&mut key).unwrap();
    let identity = Identity::from_pkcs8(&cert, &key).unwrap();
    let tls_acceptor = NativeTlsAcceptor::builder(identity).build().unwrap();
    let tls_acceptor = TlsAcceptor::from(tls_acceptor);
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    while let Ok((stream, _)) = listener.accept().await {
        let tls_acceptor = tls_acceptor.clone();
        tokio::spawn(async move {
            let peer: SocketAddr = stream.peer_addr().expect("connected streams should have a peer address");
            println!("New connection from: {}", peer);
            let tls_stream = match tls_acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    println!("TLS handshake error: {:?}", e);
                    return;
                }
            };
            let ws_stream = accept_async(tls_stream).await.expect("Error during the websocket handshake");
            let (mut write, mut read) = ws_stream.split();
            while let Some(Ok(message)) = read.next().await {
                if message.is_text() {
                    let received_text = message.into_text().unwrap();
                    if write.send(Message::Text(received_text)).await.is_err() {
                        println!("Error sending message to {}", peer);
                        break;
                    }
                }
            }
            println!("Connection closed by {}", peer);
        });
    }
}