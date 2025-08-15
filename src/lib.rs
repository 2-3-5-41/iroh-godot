use std::{collections::VecDeque, mem::replace};

use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    global::Error,
    prelude::*,
};
use iroh::{Endpoint, NodeId, node_info::NodeIdExt, protocol::Router};
use tokio::{sync::broadcast, task::JoinHandle};

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
    status: Connection,
    packet_recv: Option<broadcast::Receiver<Vec<u8>>>,
    packet_queue: VecDeque<Vec<u8>>,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    fn get_available_packet_count(&self) -> i32 {
        todo!()
    }
    fn get_max_packet_size(&self) -> i32 {
        MAX_ALLOWED_PACKET_SIZE as i32
    }
    fn get_packet_channel(&self) -> i32 {
        todo!("Need to return the most recent packet's protocol channel.")
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
        todo!()
    }
    fn get_packet_peer(&self) -> i32 {
        todo!("Get the most recent i32 peer id from most recent packet received.")
    }
    fn is_server(&self) -> bool {
        todo!()
    }
    fn poll(&mut self) {
        // Update status
        self.status = match replace(&mut self.status, Connection::Initialized) {
            Connection::Connecting(join_handle) => {
                if join_handle.is_finished() {
                    let result = match n0_future::future::block_on(join_handle) {
                        Ok(result) => result,
                        Err(err) => {
                            return godot_error!("{err}");
                        }
                    };

                    match result {
                        Ok(conn) => {
                            let rx = conn.subscribe();
                            self.packet_recv = Some(rx);
                            Connection::Connected(conn)
                        }
                        Err(err) => return godot_error!("{err}"),
                    }
                } else {
                    Connection::Connecting(join_handle)
                }
            }
            _ => Connection::Initialized,
        };

        if let Some(rx) = &mut self.packet_recv {
            match rx.try_recv() {
                Ok(packet) => self.packet_queue.push_back(packet),
                Err(err) => godot_error!("{err}"),
            }
        }
    }
    fn close(&mut self) {
        n0_future::future::block_on(self.inner.endpoint.close())
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        todo!("Disconnect peer by it's i32 id")
    }
    fn get_unique_id(&self) -> i32 {
        match &self.status {
            Connection::Connected(multiplayer_connection) => multiplayer_connection.get_id(),
            _ => 1, // Assume we are the 'server' node until we've connected to another node.
        }
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        match self.status {
            Connection::Connecting(_) => ConnectionStatus::CONNECTING,
            _ => ConnectionStatus::CONNECTED, // No matter what, we are connected to the iroh network.
        }
    }
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        todo!("Push packet to inner iroh protocols")
    }
    fn get_packet_script(&mut self) -> PackedByteArray {
        todo!("Read the most recent packet")
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    #[signal]
    fn connecting_to_peer(peer: GString);

    #[func]
    fn initialize() -> Gd<Self> {
        Gd::from_init_fn(|base| Self {
            base,
            inner: Inner::init(),
            transfer_channel: ProtocolChannel::Default,
            status: Connection::Initialized,
            packet_recv: Default::default(),
            packet_queue: Default::default(),
        })
    }

    /// Make a connection to a peer based on the current protocol channel.
    #[func]
    fn connect(&mut self, node_id: String) {
        match self.status {
            Connection::Connecting(_) => return,
            _ => (),
        }

        let raw_id = match NodeId::from_z32(&node_id) {
            Ok(id) => id,
            Err(err) => return godot_error!("{err}"),
        };

        self.status = Connection::Connecting(self.inner.multiplayer.join(raw_id));

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
        n0_future::future::block_on(async {
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

enum Connection {
    Initialized,
    Connecting(
        JoinHandle<
            Result<iroh_godot_protocol::MultiplayerConnection, iroh::endpoint::ConnectError>,
        >,
    ),
    Connected(iroh_godot_protocol::MultiplayerConnection),
}
