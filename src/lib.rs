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
use tokio::{
    sync::broadcast::{self, error::TryRecvError},
    task::JoinHandle,
};

mod godot_peer_data_generated;
mod iroh_godot_protocol;

const MAX_ALLOWED_PACKET_SIZE: usize = 512;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {}

#[derive(GodotClass)]
#[class(base = MultiplayerPeerExtension, tool, no_init)]
struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    inner: Inner,
    transfer_channel: ProtocolChannel,
    transfer_mode: TransferMode,
    status: RemoteConnection,
    connection_recv: broadcast::Receiver<(i32, Connection)>,
    packet_recv: broadcast::Receiver<Vec<u8>>,
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
        let size = MAX_ALLOWED_PACKET_SIZE as i32 - 4;
        godot_print!("[{}] Max packet size {}", self.node_id(), size);
        size
    }
    fn get_packet_channel(&self) -> i32 {
        0
    }
    fn get_packet_mode(&self) -> TransferMode {
        self.transfer_mode
    }
    fn set_transfer_channel(&mut self, p_channel: i32) {
        self.transfer_channel = ProtocolChannel::from(p_channel)
    }
    fn get_transfer_channel(&self) -> i32 {
        godot_print!(
            "[{}] Current transfer channel {:?}",
            self.node_id(),
            self.transfer_channel
        );
        self.transfer_channel.to_godot() as i32
    }
    fn set_transfer_mode(&mut self, p_mode: TransferMode) {
        if p_mode != TransferMode::RELIABLE {
            godot_warn!("QUIC only handles 'RELIABLE' streams.");
        }
        self.transfer_mode = p_mode;
    }
    fn get_transfer_mode(&self) -> TransferMode {
        godot_print!(
            "[{}] Current transfer mode {:?}",
            self.node_id(),
            self.transfer_mode
        );
        self.transfer_mode
    }
    fn set_target_peer(&mut self, p_peer: i32) {
        godot_print!("[{}] Setting target peer {}", self.node_id(), p_peer);
        self.target_peer = p_peer
    }
    fn get_packet_peer(&self) -> i32 {
        if let Some((id, _)) = self.packet_queue.front() {
            godot_print!("[{}] Getting packet peer {}", self.node_id(), id);
            return *id;
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
                    let result = match AsyncRuntime::block_on(join_handle) {
                        Ok(result) => result,
                        Err(err) => {
                            return godot_error!("{err}");
                        }
                    };

                    match result {
                        Ok(conn) => {
                            godot_print!(
                                "[{}] Connected to remote peer, and received new ID: {}",
                                self.node_id(),
                                conn.get_id()
                            );
                            self.signals().connected_to_peer().emit(conn.get_id());
                            let rx = conn.subscribe();
                            RemoteConnection::Connected(conn, rx)
                        }
                        Err(err) => return godot_error!("{err}"),
                    }
                } else {
                    RemoteConnection::Connecting(join_handle)
                }
            }
            RemoteConnection::Connected(conn, mut recv) => {
                match recv.try_recv() {
                    Ok(packet) => {
                        godot_print!("[{}] Receiving flatbuffer packet", self.node_id());
                        match root_as_multiplayer_data_packet(&packet) {
                            Ok(data_packet) => {
                                let id = data_packet.id();
                                match data_packet.packet() {
                                    Some(data) => {
                                        // Map flatbuffer vector to normal vector.
                                        let packet: Vec<u8> =
                                            data.into_iter().map(|byte| byte).collect();
                                        godot_print!(
                                            "[{}] Received flatbuffer packet from {}\nWith data {:?}",
                                            self.node_id(),
                                            id,
                                            packet.clone()
                                        );
                                        self.packet_queue.push_back((id, packet));
                                        RemoteConnection::Connected(conn, recv)
                                    }
                                    None => {
                                        godot_error!(
                                            "[{}] Could not deserialize packet into flatbuffer structure",
                                            self.node_id()
                                        );
                                        RemoteConnection::Connected(conn, recv)
                                    }
                                }
                            }
                            Err(err) => {
                                godot_error!("{err}");
                                RemoteConnection::Connected(conn, recv)
                            }
                        }
                    }
                    Err(TryRecvError::Closed) => {
                        godot_error!(
                            "[{}] Packet broadcast receiver has closed, this is a bug.",
                            self.node_id()
                        );
                        RemoteConnection::Connected(conn, recv)
                    }
                    _ => RemoteConnection::Connected(conn, recv),
                }
            }
            status => status,
        };

        // Store connections
        while let Ok((id, conn)) = self.connection_recv.try_recv() {
            self.connections.insert(id, conn);
            self.signals().peer_connected().emit(id as i64);
            godot_print!("[{}] New connection with ID: {}", self.node_id(), id);
        }

        // Receive remote flatbuffer packets
        match &mut self.packet_recv.try_recv() {
            Ok(recv) => {
                godot_print!("[{}] Receiving flatbuffer packet", self.node_id());
                match root_as_multiplayer_data_packet(&recv) {
                    Ok(data_packet) => {
                        let id = data_packet.id();
                        match data_packet.packet() {
                            Some(data) => {
                                // Map flatbuffer vector into normal vector.
                                let packet: Vec<u8> = data.into_iter().map(|byte| byte).collect();
                                godot_print!(
                                    "[{}] Received flatbuffer packet from {}\nWith data {:?}",
                                    self.node_id(),
                                    id,
                                    packet.clone()
                                );
                                self.packet_queue.push_back((id, packet));
                            }
                            None => godot_error!(
                                "[{}] Could not deserialize packet into flatbuffer structure",
                                self.node_id()
                            ),
                        };
                    }
                    Err(err) => godot_error!("[{}] {err}", self.node_id()),
                }
            }
            Err(TryRecvError::Closed) => {
                godot_error!(
                    "[{}] Packet broadcast receiver has closed, this is a bug",
                    self.node_id()
                )
            }
            _ => (),
        }
    }
    fn close(&mut self) {
        godot_print!("[{}] Closing iroh endpoint.", self.node_id());
        AsyncRuntime::block_on(self.inner.endpoint.close())
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        if let Some(disconnect) = self.connections.remove(&p_peer) {
            disconnect.close(0u8.into(), b"Disconnect request");
            self.signals().peer_disconnected().emit(p_peer as i64);
        }
    }
    fn get_unique_id(&self) -> i32 {
        match &self.status {
            RemoteConnection::Connected(multiplayer_connection, _) => {
                multiplayer_connection.get_id()
            }
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
        godot_print!("[{}] Target peer {}", self.node_id(), self.target_peer);
        match self.target_peer {
            0 => {
                godot_print!("[{}] Broadcasting packet to all peers", self.node_id());
                self.connections.iter().for_each(|(id, _)| {
                    self.inner.multiplayer.push_packet(*id, p_buffer.to_vec());
                });
            }
            1 => match &self.status {
                RemoteConnection::Initialized => return Error::ERR_SKIP,
                RemoteConnection::Connecting(_) => return Error::ERR_BUSY,
                RemoteConnection::Connected(multiplayer_connection, _) => {
                    godot_print!("[{}] Sending packet to 'server' peer", &self.node_id());
                    if let Err(err) = multiplayer_connection.push_packet(p_buffer.to_vec()) {
                        godot_error!("{err}");
                        return Error::ERR_CANT_ACQUIRE_RESOURCE;
                    }
                }
            },
            _ => {
                let id = self.target_peer;
                godot_print!("[{}] Sending packet to peer {}", &self.node_id(), id);
                self.inner.multiplayer.push_packet(id, p_buffer.to_vec());
            }
        }

        Error::OK
    }
    fn get_packet_script(&mut self) -> PackedByteArray {
        if let Some((id, packet)) = self.packet_queue.pop_front() {
            godot_print!("[{}] Reading packet from peer {}", &self.node_id(), id);
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
    fn connected_to_peer(assigned: i32);

    #[func]
    fn initialize() -> Gd<Self> {
        tracing_subscriber::fmt().init();

        let inner = Inner::init();
        let connection_recv = inner.multiplayer.connections();
        let packet_recv = inner.multiplayer.packets();

        Gd::from_init_fn(|base| Self {
            base,
            inner,
            transfer_channel: ProtocolChannel::Default,
            transfer_mode: TransferMode::RELIABLE,
            status: RemoteConnection::Initialized,
            connection_recv,
            packet_recv,
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

            let multiplayer = iroh_godot_protocol::Multiplayer::init(endpoint.clone());

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

#[derive(Debug, GodotConvert, Var, Export, Clone)]
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
        JoinHandle<Result<iroh_godot_protocol::OutboundConnection, iroh::endpoint::ConnectError>>,
    ),
    Connected(
        iroh_godot_protocol::OutboundConnection,
        broadcast::Receiver<Vec<u8>>,
    ),
}

impl Display for RemoteConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteConnection::Initialized => write!(f, "Network Initialized"),
            RemoteConnection::Connecting(_) => write!(f, "Connecting"),
            RemoteConnection::Connected(conn, _) => {
                write!(f, "Connected to remote with {:?}", conn)
            }
        }
    }
}
