use std::{
    collections::{HashMap, VecDeque},
    fmt::Display,
    mem::replace,
};

use crate::godot_peer_data_generated::root_as_multiplayer_data_packet;
use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    global::Error,
    prelude::*,
};
use godot_tokio::AsyncRuntime;
use iroh::{Endpoint, NodeId, endpoint::Connection, node_info::NodeIdExt, protocol::Router};
use tokio::{sync::broadcast, task::JoinHandle};

mod godot_peer_data_generated;
mod iroh_godot_protocol;

const MAX_ALLOWED_PACKET_SIZE: usize = 1024;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {}

#[derive(GodotClass)]
#[class(base = MultiplayerPeerExtension, tool, no_init)]
struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    inner: Inner,
    transfer_channel: ProtocolChannel,
    status: RemoteConnection,
    packet_recv: Option<broadcast::Receiver<Vec<u8>>>,
    packet_queue: VecDeque<(i32, Vec<u8>)>,
    connections: HashMap<i32, Connection>,
    target_peer: i32,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    fn get_available_packet_count(&self) -> i32 {
        self.packet_queue.len() as i32
    }
    fn get_max_packet_size(&self) -> i32 {
        MAX_ALLOWED_PACKET_SIZE as i32 - 4
    }
    fn get_packet_channel(&self) -> i32 {
        0
    }
    fn get_packet_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_transfer_channel(&mut self, p_channel: i32) {
        self.transfer_channel = ProtocolChannel::from(p_channel)
    }
    fn get_transfer_channel(&self) -> i32 {
        self.transfer_channel.to_godot() as i32
    }
    fn set_transfer_mode(&mut self, p_mode: TransferMode) {
        match p_mode {
            TransferMode::RELIABLE => return,
            _ => {
                return godot_warn!(
                    "This function does nothing; QUIC only handles 'RELIABLE' streams."
                );
            }
        }
    }
    fn get_transfer_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_target_peer(&mut self, p_peer: i32) {
        self.target_peer = p_peer
    }
    fn get_packet_peer(&self) -> i32 {
        if let Some(front) = self.packet_queue.front() {
            return front.0;
        }

        -1
    }
    fn is_server(&self) -> bool {
        if self.get_unique_id() == 1 {
            return true;
        }

        false
    }
    fn poll(&mut self) {
        // Update status
        self.status = match replace(&mut self.status, RemoteConnection::Initialized) {
            RemoteConnection::Connecting(join_handle) => {
                if join_handle.is_finished() {
                    godot_print!("Getting join handle result...");
                    let result = match AsyncRuntime::block_on(join_handle) {
                        Ok(result) => result,
                        Err(err) => {
                            return godot_error!("{err}");
                        }
                    };

                    match result {
                        Ok(conn) => {
                            godot_print!(
                                "Subscribing to packet receiver of new outgoing connection"
                            );
                            let rx = conn.subscribe();
                            self.packet_recv = Some(rx);
                            RemoteConnection::Connected(conn)
                        }
                        Err(err) => return godot_error!("{err}"),
                    }
                } else {
                    RemoteConnection::Connecting(join_handle)
                }
            }
            status => status,
        };

        // Store connections
        self.inner.multiplayer.connections().for_each(|(id, conn)| {
            self.connections.insert(id, conn);
            self.signals().peer_connected().emit(id as i64);
        });

        // Receive flatbuffer packets
        if let Some(rx) = &mut self.packet_recv {
            match rx.try_recv() {
                Ok(packet) => {
                    match root_as_multiplayer_data_packet(&packet) {
                        Ok(data_packet) => {
                            let id = data_packet.id();
                            if let Some(packet) = data_packet.packet() {
                                // Map flatbuffer vector into normal vector.
                                let packet: Vec<u8> = packet.into_iter().map(|byte| byte).collect();
                                self.packet_queue.push_back((id, packet));
                            };
                        }
                        Err(err) => godot_error!("{err}"),
                    }
                }
                Err(err) => godot_error!("{err}"),
            }
        }
    }
    fn close(&mut self) {
        n0_future::future::block_on(self.inner.endpoint.close())
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        if let Some(disconnect) = self.connections.remove(&p_peer) {
            disconnect.close(0u8.into(), b"Disconnect request");
            self.signals().peer_disconnected().emit(p_peer as i64);
        }
    }
    fn get_unique_id(&self) -> i32 {
        match &self.status {
            RemoteConnection::Connected(multiplayer_connection) => multiplayer_connection.get_id(),
            _ => 1, // Assume we are the 'server' node until we've connected to another node.
        }
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        match self.status {
            RemoteConnection::Connecting(_) => ConnectionStatus::CONNECTING,
            _ => ConnectionStatus::CONNECTED, // No matter what, we are connected to the iroh network.
        }
    }
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        match self.target_peer {
            0 => {
                self.connections.iter().for_each(|(id, _)| {
                    self.inner.multiplayer.push_packet(*id, p_buffer.to_vec());
                });
            }
            1 => match &mut self.status {
                RemoteConnection::Initialized => return Error::ERR_SKIP,
                RemoteConnection::Connecting(_) => return Error::ERR_BUSY,
                RemoteConnection::Connected(multiplayer_connection) => {
                    if let Err(err) = multiplayer_connection.push_packet(p_buffer.to_vec()) {
                        godot_error!("{err}");
                        return Error::ERR_CANT_ACQUIRE_RESOURCE;
                    }
                }
            },
            _ => {
                let id = self.target_peer;
                self.inner.multiplayer.push_packet(id, p_buffer.to_vec());
            }
        }

        Error::OK
    }
    fn get_packet_script(&mut self) -> PackedByteArray {
        if let Some((_, packet)) = self.packet_queue.pop_front() {
            return packet.into();
        }

        PackedByteArray::default()
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    #[signal]
    fn connecting_to_peer(peer: GString);
    #[signal]
    fn connected_to_peer();

    #[func]
    fn initialize() -> Gd<Self> {
        tracing_subscriber::fmt().init();
        Gd::from_init_fn(|base| Self {
            base,
            inner: Inner::init(),
            transfer_channel: ProtocolChannel::Default,
            status: RemoteConnection::Initialized,
            packet_recv: Default::default(),
            packet_queue: Default::default(),
            connections: Default::default(),
            target_peer: 0,
        })
    }

    /// Make a connection to a peer based on the current protocol channel.
    #[func]
    fn join(&mut self, node_id: String) {
        match self.status {
            RemoteConnection::Connecting(_) => return,
            _ => (),
        }

        let raw_id = match NodeId::from_z32(&node_id) {
            Ok(id) => id,
            Err(err) => return godot_error!("{err}"),
        };

        self.status = RemoteConnection::Connecting(self.inner.multiplayer.join(raw_id));

        self.signals()
            .connecting_to_peer()
            .emit(&node_id.to_godot());
    }

    /// Get a z-base32 encoded string of our local iroh node id (PublicKey).
    #[func]
    fn node_id(&self) -> String {
        let raw_id = self.inner.local_id();
        raw_id.to_z32()
    }
}

struct Inner {
    endpoint: Endpoint,
    router: Router,
    multiplayer: iroh_godot_protocol::Multiplayer,
}

impl Inner {
    fn init() -> Self {
        AsyncRuntime::block_on(async {
            let endpoint = Endpoint::builder()
                .discovery_n0()
                .bind()
                .await
                .expect("iroh runtime");

            let multiplayer = iroh_godot_protocol::Multiplayer::init(endpoint.clone()).await;

            let router = Router::builder(endpoint.clone())
                .accept(iroh_godot_protocol::ALPN, multiplayer.clone())
                .spawn();

            Self {
                endpoint,
                router,
                multiplayer,
            }
        })
    }

    fn local_id(&self) -> NodeId {
        self.router.endpoint().node_id()
    }
}

#[derive(GodotConvert, Var, Export, Clone)]
#[godot(via = i64)]
enum ProtocolChannel {
    Default = 0,
}

impl From<i32> for ProtocolChannel {
    fn from(value: i32) -> Self {
        match value {
            _ => Self::Default, // Just default to gossip if provided an invalid i32
        }
    }
}

enum RemoteConnection {
    Initialized,
    Connecting(
        JoinHandle<
            Result<iroh_godot_protocol::MultiplayerConnection, iroh::endpoint::ConnectError>,
        >,
    ),
    Connected(iroh_godot_protocol::MultiplayerConnection),
}

impl Display for RemoteConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteConnection::Initialized => write!(f, "Network Initialized"),
            RemoteConnection::Connecting(_) => write!(f, "Connecting"),
            RemoteConnection::Connected(conn) => write!(f, "Connected to remote with {:?}", conn),
        }
    }
}
