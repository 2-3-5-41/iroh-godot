use std::collections::{HashMap, VecDeque};

use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    prelude::*,
};
use iroh::{
    endpoint::{BindError, VarInt},
    protocol::{ProtocolHandler, Router},
};
use tokio::{io::AsyncReadExt, sync::mpsc::error::TryRecvError};

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {
    fn on_level_init(level: InitLevel) {
        match level {
            InitLevel::Scene => {
                // Enable multi-thread logging.
                tracing_subscriber::fmt().init();
            }
            _ => (),
        }
    }
}

#[derive(GodotClass)]
#[class(base=MultiplayerPeerExtension, tool)]
struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    runtime: tokio::runtime::Runtime,
    id: i32,
    status: ConnectionStatus,
    router_events: Option<tokio::sync::mpsc::Receiver<RouterEvents>>,
    router_connections: HashMap<i32, iroh::endpoint::Connection>,
    router_packets: VecDeque<(i32, Vec<u8>)>,
    router_target: i32,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    // Required methods
    fn get_available_packet_count(&self) -> i32 {
        self.router_packets.len() as i32
    }
    fn get_max_packet_size(&self) -> i32 {
        2048i32
    }
    fn get_packet_channel(&self) -> i32 {
        0
    }
    fn get_packet_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_transfer_channel(&mut self, p_channel: i32) {
        return godot_warn!("unused set_transfer_channel: {p_channel}");
    }
    fn get_transfer_channel(&self) -> i32 {
        0i32
    }
    fn set_transfer_mode(&mut self, p_mode: TransferMode) {
        return godot_warn!("unused set_transfer_mode: {:?}", p_mode);
    }
    fn get_transfer_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_target_peer(&mut self, p_peer: i32) {
        self.router_target = p_peer
    }
    fn get_packet_peer(&self) -> i32 {
        if let Some((front, _)) = self.router_packets.front() {
            return front.clone();
        }

        -1
    }
    fn is_server(&self) -> bool {
        self.id.eq(&1i32)
    }
    fn poll(&mut self) {
        let Some(events) = self.router_events.as_mut() else {
            return godot_warn!("Can not poll router events; missing router events channel!");
        };

        let maybe_event = match events.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                return godot_warn!("Lost connection to router events!");
            }
        };

        if let Some(event) = maybe_event {
            match event {
                RouterEvents::NewConnectionAccepted {
                    multiplayer_id,
                    connection,
                } => {
                    match self.router_connections.insert(multiplayer_id, connection) {
                        Some(prev) => godot_warn!(
                            "New router connection caused an old connection to be replaced with new: {:?}",
                            prev
                        ),
                        None => (),
                    }
                    self.signals().peer_connected().emit(multiplayer_id as i64);
                }
            }
        }
    }
    fn close(&mut self) {
        todo!()
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        match self.router_connections.remove(&p_peer) {
            Some(conn) => conn.close(VarInt::from_u32(1), b"disconnected by server"),
            None => {
                return godot_error!(
                    "There is no connection to disconnect with multiplayer id: {p_peer}"
                );
            }
        }
        self.signals().peer_disconnected().emit(p_peer as i64);
    }
    fn get_unique_id(&self) -> i32 {
        self.id
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        self.status
    }

    // Provided methods
    fn init(base: Base<MultiplayerPeerExtension>) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("tokio");
        Self {
            base,
            runtime,
            id: -1,
            status: ConnectionStatus::DISCONNECTED,
            router_events: None,
            router_connections: HashMap::default(),
            router_packets: VecDeque::with_capacity(1024),
            router_target: 0,
        }
    }
    fn get_packet_script(&mut self) -> PackedArray<u8> {
        todo!()
    }
    fn put_packet_script(&mut self, p_buffer: PackedArray<u8>) -> godot::global::Error {
        match self.router_target {
            0 => self
                .router_connections
                .iter()
                .for_each(|(id, conn)| todo!()),
            _ => if let Some(conn) = self.router_connections.get(&self.router_target) {},
        }

        godot::global::Error::OK
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    #[func]
    fn create_instance(&mut self) {
        let (tx_router_events, rx_router_events) = tokio::sync::mpsc::channel::<RouterEvents>(255);
        self.id = 1;
        self.status = ConnectionStatus::CONNECTING;
        self.runtime.spawn(init_router(tx_router_events));
        self.router_events.replace(rx_router_events);
    }
}

async fn init_router(
    tx_events: tokio::sync::mpsc::Sender<RouterEvents>,
) -> Result<Router, BindError> {
    let endpoint = iroh::Endpoint::bind().await?;
    let router = Router::builder(endpoint.clone())
        .accept(
            GodotMultiplayerProto::ALPN,
            GodotMultiplayerProto { events: tx_events },
        )
        .spawn();
    Ok(router)
}

#[derive(Debug, Clone)]
struct GodotMultiplayerProto {
    events: tokio::sync::mpsc::Sender<RouterEvents>,
}

impl ProtocolHandler for GodotMultiplayerProto {
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        let remote_multiplayer_id = {
            let mut recv_multiplayer_id = connection.accept_uni().await?;
            let id = recv_multiplayer_id.read_i32().await?;
            id
        };
        let event = RouterEvents::NewConnectionAccepted {
            multiplayer_id: remote_multiplayer_id,
            connection,
        };
        if let Err(e) = self.events.send(event).await {
            log::error!("{e}")
        }
        Ok(())
    }
}

impl GodotMultiplayerProto {
    const ALPN: &[u8] = b"godot/multiplayer/0";
}

enum RouterEvents {
    NewConnectionAccepted {
        multiplayer_id: i32,
        connection: iroh::endpoint::Connection,
    },
}
