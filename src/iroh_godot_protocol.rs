use iroh::{
    Endpoint, NodeId,
    endpoint::ConnectError,
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
    tx_send_packet: broadcast::Sender<Vec<u8>>,
    tx_recv_packet: broadcast::Sender<Vec<u8>>,
}

impl Multiplayer {
    pub fn init(endpoint: Endpoint) -> Self {
        let (tx_send_packet, _) = broadcast::channel::<Vec<u8>>(64);
        let (tx_recv_packet, _) = broadcast::channel::<Vec<u8>>(64);
        Self {
            endpoint,
            tx_send_packet,
            tx_recv_packet,
        }
    }

    pub fn join(
        &self,
        node: NodeId,
    ) -> tokio::task::JoinHandle<Result<MultiplayerConnection, ConnectError>> {
        let endpoint = self.endpoint.clone();
        n0_future::task::spawn(async move {
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
                tx_send_packet,
                tx_recv_packet,
                task,
            })
        })
    }
}
impl ProtocolHandler for Multiplayer {
    fn on_connecting(
        &self,
        connecting: iroh::endpoint::Connecting,
    ) -> impl Future<Output = Result<iroh::endpoint::Connection, iroh::protocol::AcceptError>> + Send
    {
        async move {
            let conn = connecting.await?;

            {
                // Send random i32 peer id, between 2..i32::MAX to newly connected node.
                let mut stream = conn.open_uni().await?;
                stream.write_i32(fastrand::i32(2..i32::MAX)).await?;
                stream.finish()?
            }

            Ok(conn)
        }
    }

    #[allow(refining_impl_trait)]
    fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> n0_future::boxed::BoxFuture<Result<(), AcceptError>> {
        let mut rx_send_packet = self.tx_send_packet.subscribe();
        let tx_recv_packet = self.tx_recv_packet.clone();

        Box::pin(async move {
            // Open bi-directional stream, and mpsc channels to pass around packets.
            let (mut send, mut recv) = connection.accept_bi().await?;

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
    tx_send_packet: broadcast::Sender<Vec<u8>>,
    tx_recv_packet: broadcast::Sender<Vec<u8>>,
    task: JoinHandle<()>,
}

impl MultiplayerConnection {
    pub fn get_id(&self) -> i32 {
        self.id
    }
    pub fn push_packet(
        &self,
        packet: Vec<u8>,
    ) -> Result<usize, broadcast::error::SendError<Vec<u8>>> {
        self.tx_send_packet.send(packet)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.tx_recv_packet.subscribe()
    }
}
