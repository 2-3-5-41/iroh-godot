use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    prelude::*,
};
use iroh::{
    PublicKey,
    endpoint::{BindError, VarInt},
    protocol::{ProtocolHandler, Router},
};
use std::collections::{HashMap, VecDeque};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc::error::TryRecvError,
};

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
    router_events: Option<tokio::sync::mpsc::Receiver<IrohRouterEvents>>,
    router_connections: HashMap<i32, IrohMultiplayerConnection>,
    packets: VecDeque<(i32, Vec<u8>)>,
    router_target: i32,
    router_pub_key: Option<PublicKey>,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    // Required methods
    fn get_available_packet_count(&self) -> i32 {
        self.packets.len() as i32
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
        godot_warn!("unused set_transfer_channel: {p_channel}");
    }
    fn get_transfer_channel(&self) -> i32 {
        0i32
    }
    fn set_transfer_mode(&mut self, p_mode: TransferMode) {
        godot_warn!("unused set_transfer_mode: {:?}", p_mode);
    }
    fn get_transfer_mode(&self) -> TransferMode {
        TransferMode::RELIABLE
    }
    fn set_target_peer(&mut self, p_peer: i32) {
        self.router_target = p_peer
    }
    fn get_packet_peer(&self) -> i32 {
        if let Some((front, _)) = self.packets.front() {
            return front.clone();
        }

        -1
    }
    fn is_server(&self) -> bool {
        self.id.eq(&1i32)
    }
    fn poll(&mut self) {
        // Router event process loop
        loop {
            match self.router_events.as_mut() {
                Some(receiver) => {
                    if let Ok(event) = receiver.try_recv() {
                        match event {
                            IrohRouterEvents::NewPublicId(key) => {
                                match self.router_pub_key.replace(key) {
                                    Some(old) => {
                                        godot_print!(
                                            "iroh router has initialized with new public key that replaced: {old}"
                                        );
                                        self.status = ConnectionStatus::CONNECTED
                                    }
                                    None => godot_print!("iroh router has initialized!"),
                                }
                                self.signals().public_id_changed().emit();
                            }
                            IrohRouterEvents::NewConnectionAccepted {
                                multiplayer_id,
                                connection,
                            } => {
                                let (tx_events, rx_events) =
                                    tokio::sync::mpsc::channel::<IrohConnectionEvents>(255);
                                let (tx_commands, rx_commands) =
                                    tokio::sync::mpsc::channel::<IrohConnectionCommands>(255);
                                let iroh_multiplayer_connection = IrohMultiplayerConnection {
                                    events: rx_events,
                                    commands: tx_commands,
                                };

                                self.runtime.spawn(Self::poll_connection(
                                    connection,
                                    tx_events,
                                    rx_commands,
                                ));

                                if let Some(old) = self
                                    .router_connections
                                    .insert(multiplayer_id, iroh_multiplayer_connection)
                                {
                                    godot_warn!(
                                        "A new connection removed an old connection: {:?}",
                                        old
                                    )
                                }

                                self.signals().peer_connected().emit(multiplayer_id as i64);
                            }
                        }
                    } else {
                        break;
                    }
                }
                None => {
                    godot_warn!("Cannot poll router events; missing router events channel!");
                    break;
                }
            }
        }

        self.router_connections.iter_mut().for_each(|(id, conn)| {
            while let Ok(event) = conn.get_event() {
                match event {
                    IrohConnectionEvents::RecvPacket(packet) => {
                        self.packets.push_back((id.clone(), packet))
                    }
                }
            }
        });
    }
    fn close(&mut self) {
        todo!()
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        match self.router_connections.remove(&p_peer) {
            Some(conn) => {
                if let Err(err) = conn.send_command(IrohConnectionCommands::Close) {
                    godot_error!("{err}")
                }
            }
            None => {
                return godot_error!(
                    "There is no connection to disconnect with multiplayer id: {p_peer}"
                );
            }
        }
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
            packets: VecDeque::with_capacity(1024),
            router_target: 0,
            router_pub_key: None,
        }
    }
    fn get_packet_script(&mut self) -> PackedArray<u8> {
        let Some((_, packet)) = self.packets.pop_front() else {
            godot_warn!("No packet to process...");
            return PackedByteArray::default();
        };

        packet.into()
    }
    fn put_packet_script(&mut self, p_buffer: PackedArray<u8>) -> godot::global::Error {
        let push_packet = |packet: Vec<u8>, connection: &IrohMultiplayerConnection| {
            if let Err(err) = connection.send_command(IrohConnectionCommands::PushPacket(packet)) {
                log::error!("{err}")
            }
        };

        match self.router_target {
            0 => self
                .router_connections
                .iter()
                .for_each(|(_, connection)| push_packet(p_buffer.to_vec(), connection)),
            _ => {
                if let Some(connection) = self.router_connections.get(&self.router_target) {
                    push_packet(p_buffer.to_vec(), connection)
                }
            }
        }

        godot::global::Error::OK
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    #[signal]
    fn public_id_changed();
    #[func]
    fn create_instance(&mut self) {
        let (tx_router_events, rx_router_events) =
            tokio::sync::mpsc::channel::<IrohRouterEvents>(255);
        let multiplayer_id = 1i32;
        self.id = multiplayer_id;
        self.status = ConnectionStatus::CONNECTING;
        self.runtime
            .spawn(Self::init_router(tx_router_events, multiplayer_id));
        self.router_events.replace(rx_router_events);
    }
    #[func]
    fn get_public_id(&self) -> String {
        match self.router_pub_key {
            Some(key) => key.to_string(),
            None => String::new(),
        }
    }
}

impl IrohMultiplayerPeer {
    async fn init_router(
        tx_events: tokio::sync::mpsc::Sender<IrohRouterEvents>,
        multiplayer_id: i32,
    ) -> Result<Router, BindError> {
        let endpoint = {
            let endpoint = iroh::Endpoint::bind().await?;
            if let Err(err) = tx_events
                .send(IrohRouterEvents::NewPublicId(endpoint.id()))
                .await
            {
                log::error!("{err}")
            }

            endpoint
        };

        let router = Router::builder(endpoint.clone())
            .accept(
                GodotMultiplayerProto::ALPN,
                GodotMultiplayerProto {
                    events: tx_events,
                    multiplayer_id,
                },
            )
            .spawn();

        Ok(router)
    }
    async fn poll_connection(
        connection: iroh::endpoint::Connection,
        tx_events: tokio::sync::mpsc::Sender<IrohConnectionEvents>,
        mut rx_commands: tokio::sync::mpsc::Receiver<IrohConnectionCommands>,
    ) {
        let send_packet = async |conn: &iroh::endpoint::Connection, packet: Vec<u8>| {
            // Send packet length.
            match conn.open_uni().await {
                Ok(mut send) => {
                    if let Err(err) = send.write_i32(packet.len() as i32).await {
                        log::error!("{err}")
                    }
                    if let Err(err) = send.finish() {
                        log::error!("{err}")
                    }
                }
                Err(err) => log::error!("{err}"),
            }
            // Send packet.
            match conn.open_uni().await {
                Ok(mut send) => {
                    if let Err(err) = send.write_all(packet.as_slice()).await {
                        log::error!("{err}")
                    }
                    if let Err(err) = send.finish() {
                        log::error!("{err}")
                    }
                }
                Err(err) => log::error!("{err}"),
            }
        };
        let read_packet = async |conn: &iroh::endpoint::Connection| {
            let mut size = 0usize;

            // Read packet length.
            match conn.accept_uni().await {
                Ok(mut recv) => match recv.read_i32().await {
                    Ok(len) => size = len as usize,
                    Err(err) => log::error!("{err}"),
                },
                Err(err) => log::error!("{err}"),
            }
            // Read packet.
            match conn.accept_uni().await {
                Ok(mut recv) => {
                    match recv.read_to_end(size).await {
                        Ok(packet) => return Some(packet),
                        Err(err) => {
                            log::error!("{err}");
                            return None;
                        }
                    };
                }
                Err(err) => {
                    log::error!("{err}");
                    return None;
                }
            }
        };
        loop {
            tokio::select! {
                Some(command) = rx_commands.recv() => match command {
                    IrohConnectionCommands::PushPacket(packet) => send_packet(&connection, packet).await,
                    IrohConnectionCommands::Close => {
                        connection.close(VarInt::from_u32(0), b"Connection closed by host");
                        break
                    }
                },
                Some(packet) = read_packet(&connection) => {
                    if let Err(err) = tx_events.send(IrohConnectionEvents::RecvPacket(packet)).await {
                        log::error!("{err}")
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct GodotMultiplayerProto {
    events: tokio::sync::mpsc::Sender<IrohRouterEvents>,
    multiplayer_id: i32,
}

impl ProtocolHandler for GodotMultiplayerProto {
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        // Receive remote multiplayer id, then send our own.
        let remote_multiplayer_id = {
            let (mut send_multiplayer_id, mut recv_multiplayer_id) = connection.accept_bi().await?;
            let id = recv_multiplayer_id.read_i32().await?;

            if let Err(e) = send_multiplayer_id.write_i32(self.multiplayer_id).await {
                log::error!("{e}")
            }

            id
        };
        let event = IrohRouterEvents::NewConnectionAccepted {
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

#[derive(Debug)]
struct IrohMultiplayerConnection {
    events: tokio::sync::mpsc::Receiver<IrohConnectionEvents>,
    commands: tokio::sync::mpsc::Sender<IrohConnectionCommands>,
}

impl IrohMultiplayerConnection {
    pub fn get_event(&mut self) -> Result<IrohConnectionEvents, TryRecvError> {
        self.events.try_recv()
    }
    pub fn send_command(
        &self,
        message: IrohConnectionCommands,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<IrohConnectionCommands>> {
        self.commands.try_send(message)
    }
}

enum IrohRouterEvents {
    NewPublicId(PublicKey),
    NewConnectionAccepted {
        multiplayer_id: i32,
        connection: iroh::endpoint::Connection,
    },
}

enum IrohConnectionEvents {
    RecvPacket(Vec<u8>),
}

enum IrohConnectionCommands {
    PushPacket(Vec<u8>),
    Close,
}
