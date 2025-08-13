use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
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
    Endpoint,
    endpoint::{Connection, VarInt},
    protocol::Router,
};

use crate::internal_protocol::MultiplayerPacketChannel;

mod internal_protocol;

const MAX_ALLOWED_PACKET_SIZE: usize = 1024;

type Packet = Vec<u8>;
type PeerId = i32;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {}

#[derive(GodotClass)]
#[class(base = MultiplayerPeerExtension, tool, no_init)]
struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    is_server: bool,
    internal: IrohRouterRuntime,
    peers: HashMap<PeerId, Connection>,
    packets: VecDeque<Packet>,
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
        0
    }
    fn get_packet_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_transfer_channel(&mut self, _p_channel: i32) {
        return;
    }
    fn get_transfer_channel(&self) -> i32 {
        0
    }
    fn set_transfer_mode(&mut self, _p_mode: TransferMode) {
        return;
    }
    fn get_transfer_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_target_peer(&mut self, p_peer: i32) {
        self.target_peer = p_peer;
    }
    fn get_packet_peer(&self) -> i32 {
        // TODO: Retreive a godot compatable peer-id form remote peer.
        -1
    }
    fn is_server(&self) -> bool {
        self.is_server
    }
    fn poll(&mut self) {
        self.packets = self.internal.channel.collect_incoming();
    }
    fn close(&mut self) {
        AsyncRuntime::block_on(self.internal.router().endpoint().close())
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
        -1i32
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        match self.internal.router().endpoint().is_closed() {
            true => ConnectionStatus::DISCONNECTED,
            false => ConnectionStatus::CONNECTED,
        }
    }
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        self.internal.channel.send_packet(p_buffer.to_vec());
        Error::OK
    }
    fn get_packet_script(&mut self) -> PackedByteArray {
        if let Some(packet) = self.packets.pop_front() {
            packet.into()
        } else {
            PackedByteArray::new()
        }
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    #[func]
    fn initialize_server() -> Gd<Self> {
        Gd::from_init_fn(|base| Self {
            base,
            is_server: true,
            internal: IrohRouterRuntime::new(),
            peers: Default::default(),
            packets: Default::default(),
            target_peer: Default::default(),
        })
    }

    #[func]
    fn initialize_client() -> Gd<Self> {
        Gd::from_init_fn(|base| Self {
            base,
            is_server: false,
            internal: IrohRouterRuntime::new(),
            peers: Default::default(),
            packets: Default::default(),
            target_peer: Default::default(),
        })
    }
}

struct IrohRouterRuntime {
    internal: Arc<Router>,
    channel: MultiplayerPacketChannel,
}

impl IrohRouterRuntime {
    fn new() -> Self {
        env_logger::init();

        let endpoint = AsyncRuntime::block_on(Endpoint::builder().bind()).expect("iroh endpoint");

        let (multiplayer_proto, multiplayer_packet_channel) =
            internal_protocol::GodotMultiplayer::new();

        let router = Router::builder(endpoint)
            .accept(internal_protocol::ALPN, multiplayer_proto)
            .spawn();

        let internal = Arc::new(router);

        Self {
            internal,
            channel: multiplayer_packet_channel,
        }
    }

    fn router(&self) -> Arc<Router> {
        self.internal.clone()
    }
}
