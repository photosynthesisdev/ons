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
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::sleep;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::api::setting_engine::SettingEngine;
use bytes::Bytes;
use webrtc::dtls_transport::dtls_role::DTLSRole;
use serde_json::{json, Value};
use std::fs::File;
use std::io::Write;
use csv::Writer;
use std::collections::HashMap;

// Constants for tick simulation
const CLIENT_TICK_RATE: u64 = 120; // 120 Hz
const SIMULATION_DURATION_SECS: u64 = 180; // 3 minutes

fn save_measurements(
    rtt_samples: &[u128],
    ticks: &[u64],
) -> Result<(), Box<dyn std::error::Error>> {
    // Save raw measurements in CSV format
    let mut writer = Writer::from_path("webrtc_measurements.csv")?;
    
    // Write headers
    writer.write_record(&["tick", "rtt"])?;
    
    // Write the data rows
    for i in 0..rtt_samples.len() {
        writer.write_record(&[
            ticks[i].to_string(),
            rtt_samples[i].to_string(),
        ])?;
    }
    
    writer.flush()?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut s = SettingEngine::default();
    s.set_lite(true);
    s.disable_media_engine_copy(true);
    s.set_answering_dtls_role(DTLSRole::Client);
    
    let registry = Registry::new();
    let registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .with_setting_engine(s)
        .build();

    let config = RTCConfiguration::default();
    let peer_connection = Arc::new(api.new_peer_connection(config).await?);
    
    let data_channel_mutex = Arc::new(Mutex::new(None::<Arc<RTCDataChannel>>));
    let notify = Arc::new(Notify::new());

    let pc = Arc::clone(&peer_connection);
    let dc_mutex_clone = Arc::clone(&data_channel_mutex);
    let notify_clone = Arc::clone(&notify);

    pc.on_data_channel(Box::new(move |d: Arc<RTCDataChannel>| {
        let dc_mutex_inner = Arc::clone(&dc_mutex_clone);
        let notify_inner = Arc::clone(&notify_clone);
        Box::pin(async move {
            println!("Data channel '{}' opened", d.label());
            *dc_mutex_inner.lock().await = Some(Arc::clone(&d));
            notify_inner.notify_one();
        })
    }));

    println!("Enter the SDP offer from the server:");
    let mut remote_sdp = String::new();
    stdin().read_line(&mut remote_sdp)?;
    let remote_desc: RTCSessionDescription = serde_json::from_str(&remote_sdp.trim())?;
    peer_connection.set_remote_description(remote_desc).await?;

    let answer = peer_connection.create_answer(None).await?;
    peer_connection.set_local_description(answer).await?;

    let mut gather_complete = peer_connection.gathering_complete_promise().await;
    gather_complete.recv().await;

    let local_desc = peer_connection
        .local_description()
        .await
        .ok_or(Error::new("Failed to get local description".to_string()))?;
    let sdp = serde_json::to_string(&local_desc)?;
    println!("Paste this SDP answer to the server:\n{}", sdp);

    notify.notified().await;
    
    let dc_option = data_channel_mutex.lock().await;
    let dc = dc_option.as_ref().unwrap().clone();

    let (tx, mut rx) = mpsc::channel::<Bytes>(100000);

    let tx_clone = tx.clone();
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let tx_inner = tx_clone.clone();
        Box::pin(async move {
            let _ = tx_inner.try_send(msg.data);  // Non-blocking send
        })
    }));

    // Calculate tick duration in microseconds
    let tick_duration_micros = 1_000_000 / CLIENT_TICK_RATE;
    let tick_duration = Duration::from_micros(tick_duration_micros);
    
    // Store sent tick timestamps
    let mut sent_ticks = HashMap::new();
    let mut rtt_samples = Vec::with_capacity(SIMULATION_DURATION_SECS as usize * CLIENT_TICK_RATE as usize);
    let mut recorded_ticks = Vec::with_capacity(SIMULATION_DURATION_SECS as usize * CLIENT_TICK_RATE as usize);
    
    // Initialize current tick counter
    let mut current_tick: u64 = 0;
    
    // Calculate end time
    let simulation_end_time = Instant::now() + Duration::from_secs(SIMULATION_DURATION_SECS);
    
    println!("Starting tick-based simulation at {} ticks/sec for {} seconds...", 
        CLIENT_TICK_RATE, SIMULATION_DURATION_SECS);

    // Simulation loop
    while Instant::now() < simulation_end_time {
        let tick_start = Instant::now();
        
        // Process incoming messages
        while let Ok(data) = rx.try_recv() {
            if let Ok(value) = serde_json::from_slice::<Value>(&data) {
                if let (Some(tick), Some(_timestamp)) = (value["tick"].as_u64(), value["timestamp"].as_u64()) {
                    if let Some(&sent_time) = sent_ticks.get(&tick) {
                        // Get current time in microseconds since program start
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_micros();
                        
                        let rtt = now - sent_time;
                        rtt_samples.push(rtt);
                        recorded_ticks.push(tick);
                        
                        // Print RTT for every tick
                        println!("Received tick {}, RTT: {} μs", tick, rtt);
                    }
                }
            }
        }
        
        // Send current tick
        // Use system time for consistent timestamp handling
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
            
        let message = json!({
            "tick": current_tick,
            "timestamp": now
        }).to_string();
        
        let message_bytes = Bytes::from(message.into_bytes());
        if let Err(e) = dc.send(&message_bytes).await {
            println!("Error sending tick {}: {}", current_tick, e);
        } else {
            // Store sent tick time
            sent_ticks.insert(current_tick, now);
        }
        
        // Increment tick counter
        current_tick += 1;
        
        // Yield every 100 ticks to prevent blocking
        if current_tick % 100 == 0 {
            tokio::task::yield_now().await;
        }
        
        // Sleep until next tick
        let elapsed = tick_start.elapsed();
        if elapsed < tick_duration {
            sleep(tick_duration - elapsed).await;
        } else {
            println!("Warning: Tick {} processing took longer than tick duration: {:?}", 
                current_tick - 1, elapsed);
        }
    }

    println!("\nSimulation completed!");
    
    if !rtt_samples.is_empty() {
        // Calculate statistics
        let calculate_stats = |times: &Vec<u128>| {
            let mut sorted = times.clone();
            sorted.sort_unstable();
            let min = sorted[0];
            let max = sorted[sorted.len() - 1];
            let avg = times.iter().sum::<u128>() as f64 / times.len() as f64;
            let p50 = sorted[times.len() / 2];
            let p95 = sorted[(times.len() as f64 * 0.95) as usize];
            let p99 = sorted[(times.len() as f64 * 0.99) as usize];
            (min, max, avg, p50, p95, p99)
        };

        let (min_rtt, max_rtt, avg_rtt, p50_rtt, p95_rtt, p99_rtt) = calculate_stats(&rtt_samples);
        
        println!("\nRTT Statistics:");
        println!("  Total samples: {}", rtt_samples.len());
        println!("  Min: {} µs", min_rtt);
        println!("  Max: {} µs", max_rtt);
        println!("  Average: {:.2} µs", avg_rtt);
        println!("  50th percentile: {} µs", p50_rtt);
        println!("  95th percentile: {} µs", p95_rtt);
        println!("  99th percentile: {} µs", p99_rtt);

        // Calculate message loss
        let expected_messages = current_tick;
        let received_messages = rtt_samples.len() as u64;
        let loss_rate = if expected_messages > 0 {
            (expected_messages - received_messages) as f64 / expected_messages as f64 * 100.0
        } else {
            0.0
        };
        
        println!("  Messages sent: {}", expected_messages);
        println!("  Messages received: {}", received_messages);
        println!("  Message loss rate: {:.2}%", loss_rate);

        // Print distribution of RTTs in millisecond buckets
        println!("\nRTT Distribution (1ms buckets):");
        let mut buckets = vec![0; 100]; // 0-100ms in 1ms increments
        for &rtt in &rtt_samples {
            let bucket = (rtt / 1000) as usize; // Convert to milliseconds
            if bucket < buckets.len() {
                buckets[bucket] += 1;
            }
        }
        
        for (i, count) in buckets.iter().enumerate() {
            if *count > 0 {  // Only print non-empty buckets
                println!("{}-{}ms: {} samples", i, i+1, count);
            }
        }

        // Save raw data to CSV
        save_measurements(&rtt_samples, &recorded_ticks)?;

        // Save summary statistics to JSON
        let summary = json!({
            "tick_rate": CLIENT_TICK_RATE,
            "sample_count": rtt_samples.len(),
            "messages_sent": expected_messages,
            "messages_received": received_messages,
            "loss_rate_percent": loss_rate,
            "distribution_ms": buckets.iter().enumerate()
                .filter(|(_, &count)| count > 0)
                .map(|(i, &count)| {
                    json!({
                        "bucket": format!("{}-{}", i, i+1),
                        "count": count
                    })
                })
                .collect::<Vec<_>>(),
            "rtt_micros": {
                "min": min_rtt,
                "max": max_rtt,
                "avg": avg_rtt,
                "p50": p50_rtt,
                "p95": p95_rtt,
                "p99": p99_rtt
            }
        });

        std::fs::write(
            "webrtc_summary.json",
            serde_json::to_string_pretty(&summary)?
        )?;
        
        println!("\nMeasurements saved to webrtc_measurements.csv");
        println!("Summary saved to webrtc_summary.json");
    } else {
        println!("No RTT samples collected during simulation!");
    }

    println!("Client disconnected.");
    Ok(())
}