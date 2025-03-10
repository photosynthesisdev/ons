use futures_util::{StreamExt, SinkExt};
use tokio::net::TcpListener;
use tokio_native_tls::TlsAcceptor;
use tokio_native_tls::native_tls::{Identity, TlsAcceptor as NativeTlsAcceptor};
use tokio_tungstenite::{accept_async, WebSocketStream};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio::time::{interval, Duration};
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use serde_json::{Value, from_str};
use tokio_native_tls::TlsStream;
use tokio::net::TcpStream;

// Define tick rate constants
const TICK_RATE: u32 = 30; // ticks per second
const TICK_DURATION_MICROS: u64 = 1_000_000 / TICK_RATE as u64;

// Type alias for WebSocket stream
type WsStream = WebSocketStream<TlsStream<TcpStream>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:4043".to_string();
    //let mut cert_file = File::open("/users/dorlando/ons/certs/cert1.pem")?;
    let mut cert_file = File::open("/etc/haproxy/certs/signallite_cert.pem").unwrap();
    let mut cert = vec![];
    cert_file.read_to_end(&mut cert).unwrap();
    //let mut key_file = File::open("/users/dorlando/ons/certs/privkey1.pem")?;
    let mut key_file = File::open("/etc/haproxy/certs/signallite_key.pem").unwrap();
    let mut key = vec![];
    key_file.read_to_end(&mut key).unwrap();
    
    let identity = Identity::from_pkcs8(&cert, &key)?;
    let tls_acceptor = NativeTlsAcceptor::builder(identity).build()?;
    let tls_acceptor = TlsAcceptor::from(tls_acceptor);
    
    println!("WebSocket server starting on {}", addr);
    println!("Using tick rate of {} ticks per second ({}µs per tick)", TICK_RATE, TICK_DURATION_MICROS);
    
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    println!("Server listening on {}", addr);
    
    while let Ok((stream, _)) = listener.accept().await {
        let tls_acceptor = tls_acceptor.clone();
        tokio::spawn(async move {
            let peer: SocketAddr = match stream.peer_addr() {
                Ok(addr) => addr,
                Err(e) => {
                    eprintln!("Failed to get peer address: {:?}", e);
                    return;
                }
            };
            println!("New connection from: {}", peer);
            
            // Perform TLS handshake
            let tls_stream = match tls_acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("TLS handshake error with {}: {:?}", peer, e);
                    return;
                }
            };
            
            // Perform WebSocket handshake
            let ws_stream = match accept_async(tls_stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("WebSocket handshake error with {}: {:?}", peer, e);
                    return;
                }
            };
            println!("Connection established with {}", peer);
            
            // Setup tick-based processing for this client
            handle_client(ws_stream, peer).await;
        });
    }
    
    Ok(())
}

async fn handle_client(ws_stream: WsStream, peer: SocketAddr) {
    // Split the WebSocket stream
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    
    // Wait for the first message before starting the tick loop
    println!("Waiting for first message from client {}", peer);
    let first_message = match ws_receiver.next().await {
        Some(Ok(message)) => {
            if let Ok(text) = message.into_text() {
                // Try to parse the message as JSON to get the tick number for logging
                if let Ok(parsed) = from_str::<Value>(&text) {
                    if let Some(tick) = parsed.get("tick").and_then(|t| t.as_u64()) {
                        println!("Received first message (tick {}) from {}, starting tick loop", tick, peer);
                    }
                } else {
                    println!("Received first message from {}, starting tick loop", peer);
                }
                
                // Echo the first message back immediately
                if let Err(e) = ws_sender.send(Message::Text(text.clone())).await {
                    eprintln!("Error sending first message to {}: {}", peer, e);
                    return;
                }
                
                Some(text)
            } else {
                eprintln!("First message from {} is not text", peer);
                return;
            }
        },
        Some(Err(e)) => {
            eprintln!("Error receiving first message from {}: {}", peer, e);
            return;
        },
        None => {
            println!("Client {} disconnected before sending first message", peer);
            return;
        }
    };
    
    // After receiving the first message, set up the message processing channel
    let (tx, mut rx) = mpsc::channel::<String>(100);
    
    // Shared message queue for tick processing
    let message_queue = Arc::new(Mutex::new(Vec::<String>::new()));
    let message_queue_clone = message_queue.clone();
    
    // Receiver task: process incoming WebSocket messages and add them to the queue
    let receiver_task = tokio::spawn(async move {
        while let Some(message_result) = ws_receiver.next().await {
            match message_result {
                Ok(message) => {
                    if let Ok(text) = message.into_text() {
                        // Try to parse the message as JSON to get the tick number for logging
                        if let Ok(parsed) = from_str::<Value>(&text) {
                            if let Some(tick) = parsed.get("tick").and_then(|t| t.as_u64()) {
                                println!("Received tick {} from {}", tick, peer);
                            }
                        }
                        
                        // Send the message to be processed in the next tick
                        if let Err(e) = tx.send(text).await {
                            eprintln!("Failed to send message to processing queue: {}", e);
                            break;
                        }
                    }
                },
                Err(e) => {
                    eprintln!("Error receiving message from {}: {}", peer, e);
                    break;
                }
            }
        }
        println!("Client {} disconnected", peer);
    });
    
    // Message processor: collect messages from the channel into the queue
    let processor_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let mut queue = message_queue_clone.lock().unwrap();
            queue.push(message);
        }
    });
    
    // Tick task: process messages on each tick
    let tick_task = tokio::spawn(async move {
        let tick_duration = Duration::from_micros(TICK_DURATION_MICROS);
        let mut tick_interval = interval(tick_duration);
        
        loop {
            // Wait for the next tick
            tick_interval.tick().await;
            
            // Process all messages in the queue
            let messages_to_process = {
                let mut queue = message_queue.lock().unwrap();
                let messages = queue.clone();
                queue.clear();
                messages
            };
            
            // Echo each message back
            for message in messages_to_process {
                match ws_sender.send(Message::Text(message.clone())).await {
                    Ok(_) => {
                        // Try to parse the message as JSON to get the tick number for logging
                        if let Ok(parsed) = from_str::<Value>(&message) {
                            if let Some(tick) = parsed.get("tick").and_then(|t| t.as_u64()) {
                                println!("Echoed tick {} to {}", tick, peer);
                            }
                        }
                    },
                    Err(e) => {
                        eprintln!("Error sending message to {}: {}", peer, e);
                        return;
                    }
                }
            }
        }
    });
    
    // Wait for any task to complete (which means the connection is closing)
    tokio::select! {
        _ = receiver_task => println!("Receiver task for {} completed", peer),
        _ = processor_task => println!("Processor task for {} completed", peer),
        _ = tick_task => println!("Tick task for {} completed", peer),
    }
    
    println!("Connection with {} closed", peer);
}