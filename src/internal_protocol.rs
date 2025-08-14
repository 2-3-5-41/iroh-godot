use crate::{
    MAX_ALLOWED_PACKET_SIZE,
    shared::{NewConnection, Packet, RecvPacket},
};
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, mpsc},
};

pub const ALPN: &[u8] = b"iroh/godot/0";

/// The internal pesudo protocol.
#[derive(Debug, Clone)]
pub struct GodotMultiplayer {
    incoming: Arc<mpsc::Sender<RecvPacket>>,
    outgoing: Arc<Mutex<mpsc::Receiver<Packet>>>,
    new_connections: Arc<mpsc::Sender<NewConnection>>,
}

impl GodotMultiplayer {
    pub fn new() -> (Self, MultiplayerSyncChannel) {
        let (incoming_packet_send, incoming_packet_recv) = mpsc::channel::<RecvPacket>(128);
        let (outgoing_packet_send, outgoing_packet_recv) = mpsc::channel::<Packet>(128);
        let (accepted_connections_send, accepted_connections_recv) =
            mpsc::channel::<NewConnection>(128);

        (
            Self {
                incoming: Arc::new(incoming_packet_send),
                outgoing: Arc::new(Mutex::new(outgoing_packet_recv)),
                new_connections: Arc::new(accepted_connections_send),
            },
            MultiplayerSyncChannel {
                incoming_packets: incoming_packet_recv,
                outgoing_packets: outgoing_packet_send,
                accepted_connections: accepted_connections_recv,
            },
        )
    }
}

impl ProtocolHandler for GodotMultiplayer {
    #[allow(refining_impl_trait)]
    fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> n0_future::boxed::BoxFuture<Result<(), AcceptError>> {
        let return_to_main = self.incoming.clone();
        let send_to_peer = self.outgoing.clone();
        let new_conn = self.new_connections.clone();

        Box::pin(async move {
            let (mut send, mut recv) = match connection.accept_bi().await {
                Ok(stream) => stream,
                Err(err) => return Err(err.into()),
            };

            // First thing to be sent to the new peer should always be a possitive signed
            // 32-bit intiger to be used on Godot's side as the peer id.
            let new_id = fastrand::i32(2..i32::MAX);
            if let Err(err) = send.write_i32(new_id).await {
                return Err(err.into());
            }

            // Retern the connection and it's new id to the main thread.
            if let Err(err) = new_conn.send((connection, new_id)).await {
                log::error!("{}", err)
            }

            loop {
                // Collect incoming packets.
                let incoming_buf: &mut [u8; MAX_ALLOWED_PACKET_SIZE] =
                    &mut [0u8; MAX_ALLOWED_PACKET_SIZE];

                if let Err(err) = recv.read_exact(incoming_buf).await {
                    log::error!("{}", err)
                }

                // Send received packet to main thread.
                if let Err(err) = return_to_main.send((new_id, incoming_buf.to_vec())).await {
                    log::error!("{}", err)
                }

                // Send packets received from main thread to remote peer.
                if let Some(packet) = send_to_peer.lock().await.recv().await {
                    if let Err(err) = send.write_all(packet.as_slice()).await {
                        log::error!("{}", err)
                    }
                }
            }
        })
    }
}

/// Used to send data between Godot/main thread, and iroh/tokio threads.
pub struct MultiplayerSyncChannel {
    incoming_packets: mpsc::Receiver<RecvPacket>,
    outgoing_packets: mpsc::Sender<Packet>,
    accepted_connections: mpsc::Receiver<NewConnection>,
}

impl MultiplayerSyncChannel {
    pub fn accepted_connections(&mut self) -> std::vec::IntoIter<NewConnection> {
        let mut connections: Vec<NewConnection> = Vec::new();
        while let Ok(conn) = self.accepted_connections.try_recv() {
            connections.push(conn);
        }
        connections.into_iter()
    }

    pub fn incoming_packets(&mut self) -> std::vec::IntoIter<RecvPacket> {
        let mut incoming: Vec<RecvPacket> = Vec::new();
        while let Ok(packet) = self.incoming_packets.try_recv() {
            incoming.push(packet);
        }
        incoming.into_iter()
    }

    pub fn send_packet(&self, packet: Packet) {
        if let Err(err) = self.outgoing_packets.try_send(packet) {
            log::error!("{}", err)
        }
    }
}
