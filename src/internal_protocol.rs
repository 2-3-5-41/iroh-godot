use std::{collections::VecDeque, sync::Arc};

use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::sync::{Mutex, mpsc};

use crate::{MAX_ALLOWED_PACKET_SIZE, Packet};

pub const ALPN: &[u8] = b"iroh/godot/0";

#[derive(Debug, Clone)]
pub struct GodotMultiplayer {
    incoming: Arc<mpsc::Sender<Packet>>,
    outgoing: Arc<Mutex<mpsc::Receiver<Packet>>>,
}

impl GodotMultiplayer {
    pub fn new() -> (Self, MultiplayerPacketChannel) {
        let (incoming_packet_send, incoming_packet_recv) = mpsc::channel::<Packet>(128);
        let (outgoing_packet_send, outgoing_packet_recv) = mpsc::channel::<Packet>(128);

        (
            Self {
                incoming: Arc::new(incoming_packet_send),
                outgoing: Arc::new(Mutex::new(outgoing_packet_recv)),
            },
            MultiplayerPacketChannel {
                incoming_packets: incoming_packet_recv,
                outgoing_packets: outgoing_packet_send,
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

        Box::pin(async move {
            let (mut send, mut recv) = match connection.accept_bi().await {
                Ok(stream) => stream,
                Err(err) => return Err(err.into()),
            };

            loop {
                // Collect incoming packets.
                let incoming_buf: &mut [u8; MAX_ALLOWED_PACKET_SIZE] =
                    &mut [0u8; MAX_ALLOWED_PACKET_SIZE];

                if let Err(err) = recv.read_exact(incoming_buf).await {
                    log::error!("{}", err)
                }

                // Send received packet to main thread.
                if let Err(err) = return_to_main.send(incoming_buf.to_vec()).await {
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

pub struct MultiplayerPacketChannel {
    incoming_packets: mpsc::Receiver<Packet>,
    outgoing_packets: mpsc::Sender<Packet>,
}

impl MultiplayerPacketChannel {
    pub fn collect_incoming(&mut self) -> VecDeque<Packet> {
        let mut packets: VecDeque<Packet> = VecDeque::new();
        while let Some(packet) = self.incoming_packets.try_recv().ok() {
            packets.push_back(packet);
        }
        packets
    }

    pub fn send_packet(&self, packet: Packet) {
        if let Err(err) = self.outgoing_packets.try_send(packet) {
            log::error!("{}", err)
        }
    }
}
