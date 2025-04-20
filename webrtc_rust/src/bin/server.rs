// server.rs
use std::sync::Arc;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::Error;
use std::io::stdin;
use std::time::{Instant, Duration};
use tokio::time::sleep;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::dtls_transport::dtls_role::DTLSRole;
use tokio::sync::{mpsc, Mutex};
use bytes::Bytes;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

// Constants for tick simulation
const SERVER_TICK_RATE: u64 = 32; // Ticks per second - reduced to prevent connection overload
const BUFFER_SIZE: usize = 10000; // Buffer size for data channels

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut s = SettingEngine::default();
    s.set_lite(true);
    s.disable_media_engine_copy(true);
    let _ = s.set_answering_dtls_role(DTLSRole::Server);
    // Set ICE timeouts for better reliability
    s.set_ice_timeouts(
        Some(Duration::from_secs(10)), // disconnected_timeout
        Some(Duration::from_secs(20)), // failed_timeout
        Some(Duration::from_secs(2))   // keep_alive_interval
    );
    
    // Note: We would use setters for fixed ports if they were available
    // For now, we'll use what's available in WebRTC v0.11.0
    // The XDP filter should look for all UDP traffic on ports used by WebRTC
    
    let registry = Registry::new();
    let registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .with_setting_engine(s)
        .build();

    let config = RTCConfiguration::default();

    let peer_connection = Arc::new(api.new_peer_connection(config).await?);

    let mut data_channel_init = RTCDataChannelInit::default();
    data_channel_init.ordered = Some(false);
    data_channel_init.max_retransmits = Some(0);
    data_channel_init.protocol = Some("binary".to_string());

    let data_channel = peer_connection.create_data_channel("data", Some(data_channel_init)).await?;
    
    // Create a message queue to store incoming messages
    let message_queue = Arc::new(Mutex::new(VecDeque::new()));
    
    // Flag to indicate if the server should start ticking (set to true after first message)
    let start_ticking = Arc::new(AtomicBool::new(false));
    
    // Create channels for message passing
    let (tx, mut rx) = mpsc::channel::<Bytes>(BUFFER_SIZE);

    let dc = Arc::clone(&data_channel);
    dc.on_open(Box::new(move || {
        println!("Data channel opened");
        Box::pin(async {})
    }));

    // Handle incoming messages
    let tx_clone = tx.clone();
    let dc_for_message = Arc::clone(&data_channel);
    dc_for_message.on_message(Box::new(move |msg: DataChannelMessage| {
        let tx_inner = tx_clone.clone();
        Box::pin(async move {
            let _ = tx_inner.try_send(msg.data);  // Non-blocking send
        })
    }));

    peer_connection.on_ice_connection_state_change(Box::new(|s| {
        println!("ICE Connection State has changed: {}", s);
        Box::pin(async {})
    }));

    let offer = peer_connection.create_offer(None).await?;
    peer_connection.set_local_description(offer).await?;

    let mut gather_complete = peer_connection.gathering_complete_promise().await;
    gather_complete.recv().await;

    let local_desc = peer_connection
        .local_description()
        .await
        .ok_or(Error::new("Failed to get local description".to_string()))?;
    let sdp = serde_json::to_string(&local_desc)?;
    println!("Paste this SDP offer to the client:\n{}", sdp);

    println!("Enter the SDP answer from the client:");
    let mut remote_sdp = String::new();
    stdin().read_line(&mut remote_sdp)?;
    let remote_desc: RTCSessionDescription = serde_json::from_str(&remote_sdp.trim())?;
    peer_connection.set_remote_description(remote_desc).await?;

    println!("Server running, waiting for first message to start tick simulation...");

    // Create queue for storing incoming messages
    let msg_queue = Arc::clone(&message_queue);
    let tick_flag = Arc::clone(&start_ticking);
    
    // Message processor task
    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            let mut queue = msg_queue.lock().await;
            queue.push_back(data);
            
            // Set start_ticking to true after receiving first message
            if !tick_flag.load(Ordering::SeqCst) {
                tick_flag.store(true, Ordering::SeqCst);
                println!("First message received, starting tick simulation!");
            }
        }
    });

    // Calculate tick duration in microseconds
    let tick_duration_micros = 1_000_000 / SERVER_TICK_RATE;
    let tick_duration = Duration::from_micros(tick_duration_micros);
    
    // Monitor connection state
    let pc_monitor = Arc::clone(&peer_connection);
    let monitor_tick_flag = Arc::clone(&start_ticking);
    tokio::spawn(async move {
        loop {
            let state = pc_monitor.connection_state();
            if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed ||
               state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Disconnected {
                println!("Connection state changed to: {}", state);
                println!("NOTE: WebRTC connection may have failed/disconnected. Will continue when reconnected.");
            }
            
            // Reset start_ticking if connection is closed/failed
            if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed {
                monitor_tick_flag.store(false, Ordering::SeqCst);
                println!("Connection failed. Will wait for new message to restart tick simulation.");
            }

            // If connected, log ICE transport information for XDP filtering
            if state == webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected {
                // Log local candidates for XDP filtering
                let stats = pc_monitor.get_stats().await;
                println!("WebRTC LOCAL CONNECTION INFO FOR XDP FILTERING:");
                println!("Stats: {:?}", stats);
                
                // When you see this output, look for port numbers in the stats
                // that you can target with your XDP filter
            }
            
            sleep(Duration::from_millis(1000)).await;
        }
    });
    
    // Data channel for the tick loop
    let tick_dc = Arc::clone(&data_channel);
    
    // Wait for first message before starting tick simulation
    while !start_ticking.load(Ordering::SeqCst) {
        sleep(Duration::from_millis(10)).await;
    }
    
    println!("Tick simulation started at {} ticks/sec", SERVER_TICK_RATE);
    
    let mut consecutive_empty_ticks = 0;
    
    // Tick loop
    loop {
        let tick_start = Instant::now();
        
        // Check if we should still be ticking
        if !start_ticking.load(Ordering::SeqCst) {
            // Wait for first message again
            println!("Waiting for client to reconnect and send first message...");
            while !start_ticking.load(Ordering::SeqCst) {
                sleep(Duration::from_millis(100)).await;
            }
            println!("Client reconnected! Resuming tick simulation.");
            consecutive_empty_ticks = 0;
        }
        
        // Process all queued messages
        let messages_to_process = {
            let mut queue = message_queue.lock().await;
            let msgs = queue.drain(..).collect::<Vec<_>>();
            msgs
        };
        
        // Track empty ticks for health monitoring
        if messages_to_process.is_empty() {
            consecutive_empty_ticks += 1;
            
            // If we haven't received anything for a while, log it (but keep going)
            if consecutive_empty_ticks % 100 == 0 {
                println!("No messages received for {} consecutive ticks", consecutive_empty_ticks);
            }
        } else {
            consecutive_empty_ticks = 0;
        }
        
        // Process each message
        for data in messages_to_process {
            if let Ok(value) = serde_json::from_slice::<Value>(&data) {
                if let (Some(tick), Some(timestamp)) = (value["tick"].as_u64(), value["timestamp"].as_u64()) {
                    // Print received tick
                    println!("Server received tick {} with timestamp {}", tick, timestamp);
                    
                    // Echo the message back with the same tick number and timestamp
                    let response = json!({
                        "tick": tick,
                        "timestamp": timestamp
                    }).to_string();
                    
                    let response_bytes = Bytes::from(response.into_bytes());
                    
                    // Check data channel state before sending
                    if data_channel.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                        if let Err(e) = tick_dc.send(&response_bytes).await {
                            println!("Error sending response for tick {}: {}", tick, e);
                        } else {
                            println!("Server sent response for tick {}", tick);
                        }
                    } else {
                        println!("Data channel not open, state: {}", data_channel.ready_state());
                    }
                }
            }
        }
        
        // Calculate time to sleep until next tick
        let elapsed = tick_start.elapsed();
        if elapsed < tick_duration {
            sleep(tick_duration - elapsed).await;
        } else {
            // If processing took longer than the tick duration, log a warning
            println!("Warning: Tick processing took longer than tick duration: {:?}", elapsed);
        }
        
        // Yield periodically to prevent blocking
        tokio::task::yield_now().await;
    }
}
