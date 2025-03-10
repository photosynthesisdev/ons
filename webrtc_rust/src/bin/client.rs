use std::sync::Arc;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::Error;
use std::io::stdin;
use std::time::Instant;
use tokio::sync::{Mutex, Notify, mpsc};
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::api::setting_engine::SettingEngine;
use bytes::Bytes;
use webrtc::dtls_transport::dtls_role::DTLSRole;
use serde_json::json;
use std::fs::File;
use std::io::Write;
use csv::Writer;

fn save_measurements(
    rtt_samples: &[u128],
    send_times: &[u128],
    channel_times: &[u128],
) -> Result<(), Box<dyn std::error::Error>> {
    // Save raw measurements in CSV format
    let mut writer = Writer::from_path("webrtc_measurements.csv")?;
    
    // Write headers
    writer.write_record(&["rtt", "send_time", "channel_time"])?;
    
    // Write the data rows
    for i in 0..rtt_samples.len() {
        writer.write_record(&[
            rtt_samples[i].to_string(),
            send_times[i].to_string(),
            channel_times[i].to_string(),
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

    let mut rtt_samples = Vec::with_capacity(100000);
    let mut send_to_network_times = Vec::with_capacity(100000);
    let mut channel_times = Vec::with_capacity(100000);
    let message_limit = 10000;
    
    let message = Bytes::from_static(&[1u8; 8]);

    // Warmup phase
    println!("Starting warmup...");
    for _ in 0..100 {
        dc.send(&message).await?;
        let _ = rx.recv().await;
    }

    println!("Starting RTT measurements with timing breakdown...");
    
    for i in 0..message_limit {
        let start_time = Instant::now();
        
        // Measure time to send
        let send_start = Instant::now();
        dc.send(&message).await?;
        let send_time = send_start.elapsed().as_micros();
        send_to_network_times.push(send_time);

        if let Some(_) = rx.recv().await {
            let total_rtt = start_time.elapsed().as_micros();
            let channel_time = total_rtt - send_time;
            
            rtt_samples.push(total_rtt);
            channel_times.push(channel_time);
        }

        if i % 5000 == 0 {
            tokio::task::yield_now().await;
        }
    }

    if !rtt_samples.is_empty() {
        // Calculate statistics for each timing component
        let calculate_stats = |times: &Vec<u128>| {
            let mut sorted = times.clone();
            sorted.sort_unstable();
            let avg = times.iter().sum::<u128>() as f64 / times.len() as f64;
            let p50 = sorted[times.len() / 2];
            let p95 = sorted[(times.len() as f64 * 0.95) as usize];
            let p99 = sorted[(times.len() as f64 * 0.99) as usize];
            (avg, p50, p95, p99)
        };

        println!("\nTiming Breakdown:");
        
        let (send_avg, send_p50, send_p95, send_p99) = calculate_stats(&send_to_network_times);
        println!("\nTime to send to network:");
        println!("  Average: {:.2} µs", send_avg);
        println!("  50th percentile: {} µs", send_p50);
        println!("  95th percentile: {} µs", send_p95);
        println!("  99th percentile: {} µs", send_p99);

        let (channel_avg, channel_p50, channel_p95, channel_p99) = calculate_stats(&channel_times);
        println!("\nTime in network/processing:");
        println!("  Average: {:.2} µs", channel_avg);
        println!("  50th percentile: {} µs", channel_p50);
        println!("  95th percentile: {} µs", channel_p95);
        println!("  99th percentile: {} µs", channel_p99);

        let (total_avg, total_p50, total_p95, total_p99) = calculate_stats(&rtt_samples);
        println!("\nTotal RTT:");
        println!("  Average: {:.2} µs", total_avg);
        println!("  50th percentile: {} µs", total_p50);
        println!("  95th percentile: {} µs", total_p95);
        println!("  99th percentile: {} µs", total_p99);

        // Print distribution of RTTs in smaller buckets for more detail
        println!("\nRTT Distribution (100µs buckets):");
        let mut buckets = vec![0; 50]; // 0-5000µs in 100µs increments
        for &rtt in &rtt_samples {
            let bucket = (rtt / 100) as usize;
            if bucket < buckets.len() {
                buckets[bucket] += 1;
            }
        }
        
        for (i, count) in buckets.iter().enumerate() {
            if *count > 0 {  // Only print non-empty buckets
                println!("{}-{}µs: {} samples", i*100, (i+1)*100, count);
            }
        }

        // Save raw data to CSV
        save_measurements(&rtt_samples, &send_to_network_times, &channel_times)?;

        // Save summary statistics to JSON
        let summary = json!({
            "sample_count": rtt_samples.len(),
            "distribution": buckets.iter().enumerate()
                .filter(|(_, &count)| count > 0)
                .map(|(i, &count)| {
                    json!({
                        "bucket": format!("{}-{}", i*100, (i+1)*100),
                        "count": count
                    })
                })
                .collect::<Vec<_>>(),
            "metrics": {
                "total_rtt": {
                    "avg": total_avg,
                    "p50": total_p50,
                    "p95": total_p95,
                    "p99": total_p99
                },
                "send_time": {
                    "avg": send_avg,
                    "p50": send_p50,
                    "p95": send_p95,
                    "p99": send_p99
                },
                "channel_time": {
                    "avg": channel_avg,
                    "p50": channel_p50,
                    "p95": channel_p95,
                    "p99": channel_p99
                }
            }
        });

        std::fs::write(
            "webrtc_summary.json",
            serde_json::to_string_pretty(&summary)?
        )?;
    }

    println!("Client disconnected.");
    Ok(())
}