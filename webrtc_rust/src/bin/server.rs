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
use tokio::time::{sleep, Duration};
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::dtls_transport::dtls_role::DTLSRole;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut s = SettingEngine::default();
    s.set_lite(true);
    s.disable_media_engine_copy(true);
    s.set_answering_dtls_role(DTLSRole::Server);
    
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

    let dc = Arc::clone(&data_channel);
    dc.on_open(Box::new(move || {
        println!("Data channel opened");
        Box::pin(async {})
    }));

    let dc_clone = Arc::clone(&dc);
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let dc_inner = Arc::clone(&dc_clone);
        Box::pin(async move {
            let _ = dc_inner.send(&msg.data).await;
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

    println!("Server running, ready to handle data...");

    loop {
        sleep(Duration::from_secs(1)).await;
    }
}