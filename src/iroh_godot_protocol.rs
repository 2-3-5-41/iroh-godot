use crate::{
    MAX_ALLOWED_PACKET_SIZE,
    godot_peer_data_generated::{MultiplayerDataPacket, MultiplayerDataPacketArgs},
};
use godot_tokio::AsyncRuntime;
use iroh::{
    Endpoint, NodeId,
    endpoint::{ConnectError, Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
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
    pub fn init(endpoint: Endpoint) -> Self {
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
    ) -> tokio::task::JoinHandle<Result<OutboundConnection, ConnectError>> {
        let endpoint = self.endpoint.clone();
        let tx_connections = self.connections.clone();
        AsyncRuntime::spawn(async move {
            log::info!("Getting remote endpoint connection.");
            let connection = endpoint.connect(node, ALPN).await?;

            log::info!("[Join] Opening bi-directional stream to pass godot rpc packets through.");
            // Open bi-directional stream, and mpsc channels to pass around packets.
            let mut stream = connection.open_bi().await?;

            let (id_send, _) = &mut stream;

            // Send random i32 peer id, between 2..i32::MAX to newly connected node.
            log::info!("[Join] Sending randomly generated, self assigned, i32 id to remote peer.");
            let id = fastrand::i32(2..i32::MAX);
            if let Err(err) = id_send.write_i32(id).await {
                log::error!("[Join] {err}")
            };

            // Send connection with 'server' id back to main thread.
            send_conn_to_main(tx_connections, 1, connection);

            let (tx_send_packet, rx_send_packet) = broadcast::channel::<Vec<u8>>(64);
            let (tx_recv_packet, _) = broadcast::channel::<Vec<u8>>(64);

            let tx_packet = tx_recv_packet.clone();

            log::info!(
                "[Join] Spawning async task runtime to handle sending and receiving packets."
            );
            // Create our async task that handles the connection.
            let task = AsyncRuntime::spawn(async move {
                let (mut send, mut recv) = stream;
                let (tx_recv_packet, mut rx_send_packet) = (tx_packet, rx_send_packet);

                loop {
                    select! {
                        // Send packets we receive from main thread.
                        send_packet = rx_send_packet.recv() => match send_packet {
                            Ok(to_send) => {
                                send_to_stream(id, to_send, &mut send).await;
                            }
                            Err(err) => log::error!("[Join] {err}"),
                        },
                        // Receive packets from remote peer.
                        _ = read_from_stream(&mut recv, tx_recv_packet.clone()) => {}
                    }
                }
            });

            Ok(OutboundConnection {
                id,
                send_packet: tx_send_packet,
                recv_packet: tx_recv_packet,
                _task: task,
            })
        })
    }

    pub fn connections(&self) -> broadcast::Receiver<(i32, Connection)> {
        self.connections.subscribe()
    }

    pub fn packets(&self) -> broadcast::Receiver<Vec<u8>> {
        self.recv_packet.subscribe()
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
        log::info!("[Accept] Accepted new connection, and grabbing channels");
        let mut rx_send_packet = self.send_packet.subscribe();
        let tx_recv_packet = self.recv_packet.clone();
        let tx_connections = self.connections.clone();

        Box::pin(async move {
            log::info!("[Accept] Accepting bi-directional stream with remote peer");
            let (mut send, mut recv) = connection.accept_bi().await?;

            let remote_id = recv.read_i32().await?;
            log::info!("[Accept] Received remote random i32 peer id: {}", remote_id);

            // Return connected node's randomized i32 id, and connection.
            send_conn_to_main(tx_connections, remote_id, connection);

            log::info!("[Accept] Starting runtime loop to handle incoming and outgoing packets");
            loop {
                select! {
                    // Send packets we receive from main thread.
                    send_packet = rx_send_packet.recv() => match send_packet {
                        Ok((id, to_send)) => {
                            if id != remote_id {
                                continue;
                            }
                            send_to_stream(id, to_send, &mut send).await;
                        }
                        Err(err) => log::error!("[Accept] {err}"),
                    },
                    // Receive packets from remote peer.
                    _ = read_from_stream(&mut recv, tx_recv_packet.clone()) => {}
                }
            }
        })
    }
}

#[derive(Debug)]
pub struct OutboundConnection {
    id: i32,
    send_packet: broadcast::Sender<Vec<u8>>,
    recv_packet: broadcast::Sender<Vec<u8>>,
    _task: JoinHandle<()>,
}

impl OutboundConnection {
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

fn create_flatbuffer<'a>(id: i32, godot_packet: Vec<u8>) -> flatbuffers::FlatBufferBuilder<'a> {
    let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(MAX_ALLOWED_PACKET_SIZE);

    let data = fbb.create_vector(godot_packet.as_slice());

    let fbb_packet = MultiplayerDataPacket::create(
        &mut fbb,
        &MultiplayerDataPacketArgs {
            id,
            packet: Some(data),
        },
    );

    fbb.finish(fbb_packet, None);
    fbb
}

fn send_conn_to_main(
    tx_connections: broadcast::Sender<(i32, Connection)>,
    id: i32,
    connection: Connection,
) {
    match tx_connections.send((id, connection)) {
        Ok(i) => log::info!("Sent connection back to main thread: {}", i),
        Err(err) => log::error!("{err}"),
    }
}

async fn read_from_stream(
    stream: &mut RecvStream,
    sender: broadcast::Sender<Vec<u8>>,
) -> Option<usize> {
    let buf: &mut [u8] = &mut [0u8; crate::MAX_ALLOWED_PACKET_SIZE];
    match stream.read(buf).await {
        Ok(size) => match size {
            Some(n) => {
                log::info!("Read {n}bytes from stream");
                if n > crate::MAX_ALLOWED_PACKET_SIZE {
                    log::warn!("Packet size exceeds maximum allowed size");
                    return None;
                }
                match sender.send(buf.to_vec()) {
                    Ok(n) => {
                        log::info!("Packet received successfully");
                        return Some(n);
                    }
                    Err(err) => {
                        log::error!("{err}");
                        return None;
                    }
                }
            }
            None => None,
        },
        Err(err) => {
            log::error!("{err}");
            None
        }
    }
}

async fn send_to_stream(id: i32, to_send: Vec<u8>, send: &mut SendStream) {
    log::info!("Creating flatbuffer");
    let fbb = create_flatbuffer(id, to_send);

    let finished = fbb.finished_data();

    log::info!("Sending packet of size {}bytes", finished.len());

    match send.write_all(finished).await {
        Ok(_) => log::info!("Packet sent successfully"),
        Err(err) => log::error!("{err}"),
    }
}
