// SPDX-License-Identifier: GPL-3.0

use cosmic_ext_webcam_effects_applet::dbus::{WebcamEffectsInterface, DBUS_NAME, DBUS_PATH};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("Starting webcam effects daemon");

    let (interface, status_rx) = WebcamEffectsInterface::new();

    let conn = zbus::connection::Builder::session()?
        .name(DBUS_NAME)?
        .serve_at(DBUS_PATH, interface)?
        .build()
        .await?;

    tracing::info!("D-Bus service registered: {DBUS_NAME}");

    // Now that the connection is built with the interface registered,
    // get a reference to the interface and start the signal relay.
    let iface_ref = conn
        .object_server()
        .interface::<_, WebcamEffectsInterface>(DBUS_PATH)
        .await?;
    iface_ref.get().await.spawn_relay(conn.clone(), status_rx);

    // Run forever — the panel kills us when no longer needed, or D-Bus
    // activation restarts us on the next method call.
    std::future::pending::<()>().await;
    Ok(())
}
