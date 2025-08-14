use crate::{
    internal_protocol::MultiplayerSyncChannel,
    shared::{PeerId, RecvPacket},
};
use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    global::Error,
    prelude::*,
};
use godot_tokio::AsyncRuntime;
use iroh::{
    Endpoint, NodeId,
    endpoint::{Connection, VarInt},
    protocol::Router,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

mod internal_protocol;
mod shared;

const MAX_ALLOWED_PACKET_SIZE: usize = 1024;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {}

#[derive(GodotClass)]
#[class(base = MultiplayerPeerExtension, tool, no_init)]
struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    id: PeerId,
    inner: Internal,
    peers: HashMap<PeerId, Connection>,
    packets: VecDeque<RecvPacket>,
    target_peer: PeerId,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    fn get_available_packet_count(&self) -> i32 {
        self.packets.len() as i32
    }
    fn get_max_packet_size(&self) -> i32 {
        MAX_ALLOWED_PACKET_SIZE as i32
    }
    fn get_packet_channel(&self) -> i32 {
        // TODO: return the protocol 'channel' the most recent packet was received from.
        0
    }
    fn get_packet_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_transfer_channel(&mut self, _p_channel: i32) {
        // TODO: Use this to dictate which protocol to send a packet through;
        // i.e:
        // 1 -> This lib's internal protocol
        // 2 -> `iroh-gossip`
        // 3 -> ect...
        return;
    }
    fn get_transfer_channel(&self) -> i32 {
        // TODO: Return the 'channel' number for the desired iroh protocol.
        0
    }
    fn set_transfer_mode(&mut self, _p_mode: TransferMode) {
        // Do nothing, iroh uses QUIC under the hood, and there are
        // no plans for this library to support raw UDP packets.
        return godot_warn!("This function does nothing; QUIC only handles 'RELIABLE' streams.");
    }
    fn get_transfer_mode(&self) -> TransferMode {
        godot_warn!("QUIC only speaks in 'RELIABLE' streams of data.");
        TransferMode::RELIABLE
    }
    fn set_target_peer(&mut self, p_peer: i32) {
        self.target_peer = p_peer;
    }
    fn get_packet_peer(&self) -> i32 {
        if let Some((id, _)) = self.packets.front() {
            return *id;
        }

        -1
    }
    fn is_server(&self) -> bool {
        match self.id {
            1 => true,
            _ => false,
        }
    }
    fn poll(&mut self) {
        self.handle_new_connections();
        self.handle_incoming_packets();
    }
    fn close(&mut self) {
        AsyncRuntime::block_on(self.inner.router().endpoint().close())
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        match self.peers.get(&p_peer) {
            Some(peer) => {
                peer.close(VarInt::from_u32(0), b"Disconnection request");
                self.base_mut()
                    .signals()
                    .peer_disconnected()
                    .emit(p_peer as i64);
            }
            None => return,
        }
    }
    fn get_unique_id(&self) -> i32 {
        self.id
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        match self.inner.router().endpoint().is_closed() {
            true => ConnectionStatus::DISCONNECTED,
            false => ConnectionStatus::CONNECTED,
        }
    }
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        self.inner.channel.send_packet(p_buffer.to_vec());
        Error::OK
    }
    fn get_packet_script(&mut self) -> PackedByteArray {
        if let Some((_, packet)) = self.packets.pop_front() {
            packet.into()
        } else {
            PackedByteArray::new()
        }
    }
    fn is_server_relay_supported(&self) -> bool {
        true
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    #[func]
    fn initialize() -> Gd<Self> {
        let inner = Internal::bootstrap();
        Gd::from_init_fn(|base| Self {
            base,
            id: 1,
            inner,
            peers: Default::default(),
            packets: Default::default(),
            target_peer: Default::default(),
        })
    }

    #[func]
    fn initialize_and_join() -> Gd<Self> {
        let inner = Internal::bootstrap();
        Gd::from_init_fn(|base| Self {
            base,
            id: 2,
            inner,
            peers: Default::default(),
            packets: Default::default(),
            target_peer: Default::default(),
        })
    }

    /// Get a Base32 envoded string of our node id.
    #[func]
    fn node_id(&self) -> String {
        self.inner.who_am_i()
    }
}

impl IrohMultiplayerPeer {
    fn handle_new_connections(&mut self) {
        let connections = self.inner.channel.accepted_connections();
        if connections.len() == 0 {
            return;
        }

        connections.for_each(|(conn, id)| {
            self.peers.insert(id, conn);
        });
    }
    fn handle_incoming_packets(&mut self) {
        let packets = self.inner.channel.incoming_packets();
        if packets.len() == 0 {
            return;
        }

        packets.for_each(|packet| {
            self.packets.push_back(packet);
        });
    }
}

struct Internal {
    node_id: NodeId,
    inner: Arc<Router>,
    channel: MultiplayerSyncChannel,
}

impl Internal {
    fn bootstrap() -> Self {
        env_logger::init();

        let endpoint = AsyncRuntime::block_on(Endpoint::builder().discovery_n0().bind())
            .expect("iroh endpoint");

        let node_id = endpoint.node_id();

        let (multiplayer_proto, channel) = internal_protocol::GodotMultiplayer::new();

        let router = Router::builder(endpoint)
            .accept(internal_protocol::ALPN, multiplayer_proto)
            .spawn();

        let inner = Arc::new(router);

        Self {
            node_id,
            inner,
            channel,
        }
    }

    fn who_am_i(&self) -> String {
        data_encoding::BASE32_NOPAD
            .encode(self.node_id.as_bytes())
            .to_ascii_lowercase()
    }

    fn router(&self) -> Arc<Router> {
        self.inner.clone()
    }
}
