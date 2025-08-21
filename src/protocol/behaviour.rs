use godot_tokio::AsyncRuntime;
use iroh::{
    Endpoint, NodeId,
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::broadcast,
};

use crate::protocol::connections::RemoteConnection;

#[derive(Debug, Clone)]
pub struct GodotMultiplayerProto {
    unique_id: i32,
    endpoint: Endpoint,
    inbound_connections: broadcast::Sender<(i32, RemoteConnection)>,
}

impl GodotMultiplayerProto {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            unique_id: fastrand::i32(1..i32::MAX),
            endpoint,
            inbound_connections: broadcast::channel(32).0,
        }
    }
    pub fn get_unique_id(&self) -> i32 {
        self.unique_id
    }
    pub fn subscribe_inbound_connections(&self) -> broadcast::Receiver<(i32, RemoteConnection)> {
        self.inbound_connections.subscribe()
    }
    pub fn connect(&self, node: NodeId) -> tokio::task::JoinHandle<anyhow::Result<i32>> {
        let endpoint = self.endpoint.clone();
        let unique_id = self.unique_id;
        let inbound_connections = self.inbound_connections.clone();
        AsyncRuntime::spawn(async move {
            log::info!("Connecting to remote node");
            let connection = endpoint.connect(node, super::ALPN).await?;
            let remote_node_id = connection.remote_node_id()?;

            log::info!("Opening bi-directional stream with outbound connection");
            let (mut send, mut recv) = connection.open_bi().await?;

            send.write_i32(unique_id).await?;
            log::info!(
                "Sending Godot Multiplayer API compatable peer ID {unique_id} to remote peer"
            );

            let remote_id = recv.read_i32().await?;
            log::info!("Received remote's Godot Multiplayer API compatable peer ID {remote_id}");

            log::info!("Passing inbound connection object back to main thread.");
            let connection =
                RemoteConnection::new(unique_id, remote_node_id, connection, (send, recv));
            inbound_connections.send((remote_id, connection))?;

            Ok(remote_id)
        })
    }
}

impl ProtocolHandler for GodotMultiplayerProto {
    #[allow(refining_impl_trait)]
    fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> n0_future::boxed::BoxFuture<Result<(), AcceptError>> {
        let connections = self.inbound_connections.clone();
        let unique_id = self.unique_id;
        Box::pin(async move {
            let remote_node_id = connection.remote_node_id()?;

            log::info!("Accepting bi-directional stream with incoming connection");
            let (mut send, mut recv) = connection.accept_bi().await?;

            let remote_id = recv.read_i32().await?;
            log::info!("Received remote's Godot Multiplayer API compatable peer ID {remote_id}");

            log::info!(
                "Sending Godot Multiplayer API compatable peer ID {unique_id} to remote peer"
            );
            send.write_i32(unique_id).await?;

            log::info!("Passing inbound connection object back to main thread.");
            let connection =
                RemoteConnection::new(unique_id, remote_node_id, connection, (send, recv));
            if let Err(err) = connections.send((remote_id, connection)) {
                log::error!("Error sending incoming connection object back to main thread:\n{err}")
            }
            Ok(())
        })
    }
}
