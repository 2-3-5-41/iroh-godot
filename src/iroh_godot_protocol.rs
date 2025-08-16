use godot_tokio::AsyncRuntime;
use iroh::{
    Endpoint, NodeId,
    endpoint::{ConnectError, Connection},
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::broadcast,
    task::JoinHandle,
};

pub const ALPN: &[u8] = b"iroh/godot/0";

#[derive(Debug, Clone)]
pub struct Multiplayer {
    endpoint: Endpoint,
    send_packet: broadcast::Sender<(i32, Vec<u8>)>,
    recv_packet: broadcast::Sender<Vec<u8>>,
    connections: broadcast::Sender<(i32, Connection)>,
}

impl Multiplayer {
    pub async fn init(endpoint: Endpoint) -> Self {
        let (tx_send_packet, _) = broadcast::channel::<(i32, Vec<u8>)>(64);
        let (tx_recv_packet, _) = broadcast::channel::<Vec<u8>>(64);
        let (tx_connections, _) = broadcast::channel::<(i32, Connection)>(64);
        Self {
            endpoint,
            send_packet: tx_send_packet,
            recv_packet: tx_recv_packet,
            connections: tx_connections,
        }
    }

    pub fn join(
        &self,
        node: NodeId,
    ) -> tokio::task::JoinHandle<Result<MultiplayerConnection, ConnectError>> {
        let endpoint = self.endpoint.clone();
        AsyncRuntime::spawn(async move {
            let conn = endpoint.connect(node, ALPN).await?;

            // Receive random i32 peer id from 'server' node.
            let mut stream = conn.accept_uni().await?;
            let id = stream.read_i32().await.expect("Randomly assigned i32 id");

            // Open bi-directional stream, and mpsc channels to pass around packets.
            let stream = conn.accept_bi().await?;
            let (tx_send_packet, rx_send_packet) = broadcast::channel::<Vec<u8>>(64);
            let (tx_recv_packet, _) = broadcast::channel::<Vec<u8>>(64);

            let tx_packet = tx_recv_packet.clone();
            // Create our async task that handles the connection.
            let task = n0_future::task::spawn(async move {
                let (mut send, mut recv) = stream;
                let (tx_recv_packet, mut rx_send_packet) = (tx_packet, rx_send_packet);

                loop {
                    // Send packets we receive from main thread.
                    match rx_send_packet.recv().await {
                        Ok(to_send) => {
                            if let Err(err) = send.write_all(to_send.as_slice()).await {
                                log::error!("{err}")
                            }
                        }
                        Err(err) => log::error!("{err}"),
                    }

                    // Receive packets from remote peers.
                    match recv.read_to_end(crate::MAX_ALLOWED_PACKET_SIZE).await {
                        Ok(packet) => {
                            if let Err(err) = tx_recv_packet.send(packet) {
                                log::error!("{err}");
                            }
                        }
                        Err(err) => log::error!("{err}"),
                    }
                }
            });

            Ok(MultiplayerConnection {
                id,
                send_packet: tx_send_packet,
                recv_packet: tx_recv_packet,
                _task: task,
            })
        })
    }

    pub fn connections(&self) -> std::vec::IntoIter<(i32, Connection)> {
        let mut connections: Vec<(i32, Connection)> = Vec::new();
        let mut rx = self.connections.subscribe();

        while let Ok(conn) = rx.try_recv() {
            connections.push(conn);
        }

        connections.into_iter()
    }

    pub fn push_packet(&self, id: i32, packet: Vec<u8>) {
        if let Err(err) = self.send_packet.send((id, packet)) {
            log::error!("{err}")
        }
    }
}
impl ProtocolHandler for Multiplayer {
    #[allow(refining_impl_trait)]
    fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> n0_future::boxed::BoxFuture<Result<(), AcceptError>> {
        let mut rx_send_packet = self.send_packet.subscribe();
        let tx_recv_packet = self.recv_packet.clone();
        let tx_connections = self.connections.clone();

        Box::pin(async move {
            let remote_id = {
                // Send random i32 peer id, between 2..i32::MAX to newly connected node.
                let new_id = fastrand::i32(2..i32::MAX);
                let mut stream = connection.open_uni().await?;
                stream.write_i32(new_id).await?;
                stream.finish()?;

                new_id
            };

            let (mut send, mut recv) = connection.accept_bi().await?;

            // Return connected node's randomized i32 id, and connection.
            if let Err(err) = tx_connections.send((remote_id, connection)) {
                log::error!("{err}")
            };

            loop {
                // Send packets we receive from main thread.
                match rx_send_packet.recv().await {
                    Ok((id, to_send)) => {
                        if id != remote_id {
                            continue;
                        }
                        if let Err(err) = send.write_all(to_send.as_slice()).await {
                            log::error!("{err}")
                        }
                    }
                    Err(err) => log::error!("{err}"),
                }

                // Receive packets from remote peers.
                let incoming_buf: &mut [u8; crate::MAX_ALLOWED_PACKET_SIZE] =
                    &mut [0u8; crate::MAX_ALLOWED_PACKET_SIZE];

                match recv.read_exact(incoming_buf).await {
                    Ok(()) => {
                        if incoming_buf.len() > crate::MAX_ALLOWED_PACKET_SIZE {
                            log::warn!("Ignoring oversized packet...");
                            continue;
                        }
                        if let Err(err) = tx_recv_packet.send(Vec::new()) {
                            log::error!("{err}");
                        }
                    }
                    Err(err) => log::error!("{err}"),
                }
            }
        })
    }
}

pub struct MultiplayerConnection {
    id: i32,
    send_packet: broadcast::Sender<Vec<u8>>,
    recv_packet: broadcast::Sender<Vec<u8>>,
    _task: JoinHandle<()>,
}

impl MultiplayerConnection {
    pub fn get_id(&self) -> i32 {
        self.id
    }
    pub fn push_packet(
        &self,
        packet: Vec<u8>,
    ) -> Result<usize, broadcast::error::SendError<Vec<u8>>> {
        self.send_packet.send(packet)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.recv_packet.subscribe()
    }
}
