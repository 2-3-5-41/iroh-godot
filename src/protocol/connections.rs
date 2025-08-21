use godot_tokio::AsyncRuntime;
use iroh::{
    NodeId,
    endpoint::{Connection, RecvStream, SendStream},
    node_info::NodeIdExt,
};
use tokio::{select, sync::broadcast};

use crate::godot_peer_data_generated::{MultiplayerDataPacket, MultiplayerDataPacketArgs};

pub const MAX_PACKET_SIZE: usize = 256;

#[derive(Debug, Clone)]
pub struct RemoteConnection {
    node_id: NodeId,
    connection: Connection,
    outbound: broadcast::Sender<Vec<u8>>,
    inbound: broadcast::Sender<Vec<u8>>,
}

impl RemoteConnection {
    pub fn new(
        host_id: i32,
        node_id: NodeId,
        connection: Connection,
        stream: (SendStream, RecvStream),
    ) -> Self {
        let (send_inbound, _) = broadcast::channel::<Vec<u8>>(128);
        let (send_outbound, recv_outbound) = broadcast::channel::<Vec<u8>>(128);

        log::info!("Spawning inbound conneciton runtime");
        AsyncRuntime::spawn(inbound_connection_runtime(
            host_id,
            send_inbound.clone(),
            recv_outbound,
            stream,
        ));

        Self {
            node_id,
            connection,
            outbound: send_outbound,
            inbound: send_inbound,
        }
    }
    pub fn get_node_id(&self) -> String {
        self.node_id.to_z32()
    }
    pub fn get_connection(&self) -> &Connection {
        &self.connection
    }
    pub fn push_packet(&self, packet: Vec<u8>) {
        if let Err(err) = self.outbound.send(packet) {
            log::error!("{err}")
        }
    }
    pub fn subscribe_inbound_packets(&self) -> broadcast::Receiver<Vec<u8>> {
        self.inbound.subscribe()
    }
}

async fn inbound_connection_runtime(
    host_id: i32,
    inbound_packet_channel: broadcast::Sender<Vec<u8>>,
    mut outbound_packet_channel: broadcast::Receiver<Vec<u8>>,
    mut stream: (SendStream, RecvStream),
) {
    loop {
        select! {
            inbound = handle_inbound_packets(&mut stream.1) => if let Err(err) = inbound_packet_channel.send(inbound) {
                log::error!("{err}")
            },
            outbound = outbound_packet_channel.recv() => match outbound {
                Ok(packet) => {
                    handle_outbound_packets(host_id, packet, &mut stream.0).await
                },
                Err(err) => log::error!("{err}"),
            }
        }
    }
}

async fn handle_inbound_packets(stream: &mut RecvStream) -> Vec<u8> {
    let buf: &mut [u8] = &mut [0u8; MAX_PACKET_SIZE];
    log::info!("Reading from stream");
    if let Err(err) = stream.read(buf).await {
        log::error!("{err}")
    };

    log::info!("Read {}bytes from stream", buf.len());
    buf.to_vec()
}

async fn handle_outbound_packets(host_id: i32, packet: Vec<u8>, stream: &mut SendStream) {
    log::info!("Creating flat buffer packet");
    let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(MAX_PACKET_SIZE);
    let godot_packet = fbb.create_vector(packet.as_slice());
    let packet = MultiplayerDataPacket::create(
        &mut fbb,
        &MultiplayerDataPacketArgs {
            sender: host_id,
            packet: Some(godot_packet),
        },
    );
    fbb.finish(packet, None);

    log::info!("Sending flatbuffer packet to remote peer");
    if let Err(err) = stream.write_all(fbb.finished_data()).await {
        log::error!("{err}")
    };
}
