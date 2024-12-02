use anyhow::Result;
use std::time::Duration;
use tracing::{error, info, info_span};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::EnvFilter;
use wtransport::endpoint::IncomingSession;
use wtransport::{Endpoint, Identity, ServerConfig};

use tracing::Instrument; // Import Instrument trait

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let config = ServerConfig::builder()
        .with_bind_default(4433)
        .with_identity(Identity::self_signed(["localhost"]).unwrap())
        .keep_alive_interval(Some(Duration::from_secs(3)))
        .build();
    let server = Endpoint::server(config)?;
    info!("Server ready!");
    for id in 0.. {
        let incoming_session = server.accept().await;
        tokio::spawn(handle_connection(incoming_session).instrument(info_span!("Connection", id)));
    }
    Ok(())
}

async fn handle_connection(incoming_session: IncomingSession) {
    let result = handle_connection_impl(incoming_session).await;
    error!("{:?}", result);
}

async fn handle_connection_impl(incoming_session: IncomingSession) -> Result<()> {
    info!("Waiting for session request...");
    let session_request = incoming_session.await?;
    let authority = session_request.authority().to_string();
    let path = session_request.path().to_string();
    let connection = session_request.accept().await?;

    info!("New session: Authority: '{}', Path: '{}'", authority, path);

    tokio::spawn(async move {
        loop {
            match connection.receive_datagram().await {
                Ok(datagram) => {
                    let payload = datagram.payload().to_vec(); // Store payload in a variable with a longer lifespan
                    let received_data = String::from_utf8_lossy(&payload);
                    //info!("Received datagram from client: '{}'", received_data);

                    if let Err(e) = connection.send_datagram(&payload) {
                        error!("Failed to send datagram: {:?}", e);
                    } else {
                        //info!("Echoed datagram back to client");
                    }
                }
                Err(e) => {
                    error!("Error receiving datagram: {:?}", e);
                    break;
                }
            }
        }
    });

    Ok(())
}

fn init_logging() {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_target(true)
        .with_level(true)
        .with_env_filter(env_filter)
        .init();
}
