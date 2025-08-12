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
};
use tokio::sync::mpsc::{self};

const ALPN: &[u8] = b"iroh/godot/0";
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
    internal: IrohEndpointRuntime,
    peers: HashMap<PeerId, IrohConnectionRuntime>,
    packets: VecDeque<(PeerId, Packet)>,
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
        if let Some(front) = self.packets.front() {
            front.0
        } else {
            godot_warn!("There are no packets to get most recent peer ID...");
            0
        }
    }
    fn is_server(&self) -> bool {
        self.is_server
    }
    fn poll(&mut self) {
        // Open bi-directional streams on new connections.
        self.internal
            .gather_connections()
            .into_iter()
            .for_each(|conn| {
                let new_peer_id = fastrand::i32(1..i32::MAX);

                match self
                    .peers
                    .insert(new_peer_id, IrohConnectionRuntime::new(conn))
                {
                    _ => self
                        .base_mut()
                        .signals()
                        .peer_connected()
                        .emit(new_peer_id as i64),
                }
            });

        // Store packets received from all peers.
        self.peers.iter_mut().for_each(|(id, peer)| {
            let Some(packet) = peer.recv_packet() else {
                return;
            };

            self.packets.push_back((*id, packet));
        });
    }
    fn close(&mut self) {
        AsyncRuntime::block_on(self.internal.endpoint().close())
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        match self.peers.get(&p_peer) {
            Some(peer) => {
                peer.get_connection()
                    .close(VarInt::from_u32(0), b"Disconnection request");
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
        match self.internal.endpoint().is_closed() {
            true => ConnectionStatus::DISCONNECTED,
            false => ConnectionStatus::CONNECTED,
        }
    }
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        self.peers.iter().for_each(|(_, peer)| {
            if let Err(err) = peer.send_packet(p_buffer.to_vec()) {
                godot_error!("{}", err)
            }
        });
        Error::OK
    }
    fn get_packet_script(&mut self) -> PackedByteArray {
        if let Some(packet) = self.packets.pop_front() {
            packet.1.into()
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
            internal: IrohEndpointRuntime::new(),
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
            internal: IrohEndpointRuntime::new(),
            peers: Default::default(),
            packets: Default::default(),
            target_peer: Default::default(),
        })
    }
}

struct IrohEndpointRuntime {
    internal: Arc<Endpoint>,
    recv_conn: mpsc::Receiver<Connection>,
}

impl IrohEndpointRuntime {
    fn new() -> Self {
        env_logger::init();

        let endpoint =
            AsyncRuntime::block_on(Endpoint::builder().alpns(vec![ALPN.to_vec()]).bind())
                .expect("iroh endpoint");
        let internal = Arc::new(endpoint);

        let internal_ref = internal.clone();
        let (send_conn, recv_conn) = mpsc::channel::<Connection>(128);
        AsyncRuntime::spawn(async move {
            // Loops until endpoint is closed.
            while let Some(incoming) = internal_ref.accept().await {
                let connecting = match incoming.accept() {
                    Ok(conn) => conn,
                    Err(err) => {
                        log::warn!("{}", err);
                        continue;
                    }
                };

                let connection = match connecting.await {
                    Ok(conn) => conn,
                    Err(err) => {
                        log::error!("{}", err);
                        continue;
                    }
                };

                // Send connection back to main thread.
                match send_conn.send(connection).await {
                    Err(err) => {
                        log::error!("{}", err);
                        continue;
                    }
                    _ => continue,
                }
            }
        });

        Self {
            internal,
            recv_conn,
        }
    }

    fn gather_connections(&mut self) -> VecDeque<Connection> {
        let mut connections: VecDeque<Connection> = VecDeque::new();
        while let Ok(conn) = self.recv_conn.try_recv() {
            connections.push_back(conn);
        }
        connections
    }

    fn endpoint(&self) -> Arc<Endpoint> {
        self.internal.clone()
    }
}

struct IrohConnectionRuntime {
    connection: Arc<Connection>,
    packet_recv: tokio::sync::mpsc::Receiver<Packet>,
    packet_send: tokio::sync::mpsc::Sender<Packet>,
}

impl IrohConnectionRuntime {
    fn new(connection: Connection) -> Self {
        let connection = Arc::new(connection);
        let (in_packet_send, in_packet_recv) = mpsc::channel::<Packet>(128);
        let (out_packet_send, mut out_packet_recv) = mpsc::channel::<Packet>(128);

        let conn_ref = connection.clone();
        AsyncRuntime::spawn(async move {
            let (mut send, mut recv) = match conn_ref.accept_bi().await {
                Ok(stream) => stream,
                Err(err) => return log::error!("{}", err),
            };

            loop {
                // handle incoming packets
                let incoming_buf: &mut [u8; MAX_ALLOWED_PACKET_SIZE] =
                    &mut [0u8; MAX_ALLOWED_PACKET_SIZE];

                match recv.read_exact(incoming_buf).await {
                    Err(err) => {
                        log::error!("{}", err);
                    }
                    _ => {
                        // Send received packet to main thread.
                        if let Err(err) = in_packet_send.send(incoming_buf.to_vec()).await {
                            log::error!("{}", err)
                        }
                    }
                }

                // handle outgoing packets
                match out_packet_recv.recv().await {
                    Some(packet) => {
                        if let Err(err) = send.write_all(packet.as_slice()).await {
                            log::error!("{}", err)
                        }
                    }
                    None => continue,
                }

                continue;
            }
        });

        Self {
            connection,
            packet_recv: in_packet_recv,
            packet_send: out_packet_send,
        }
    }

    fn get_connection(&self) -> Arc<Connection> {
        self.connection.clone()
    }

    fn recv_packet(&mut self) -> Option<Packet> {
        match self.packet_recv.try_recv() {
            Ok(packet) => Some(packet),
            Err(err) => {
                eprintln!("{}", err);
                None
            }
        }
    }

    fn send_packet(&self, packet: Packet) -> Result<(), mpsc::error::TrySendError<Packet>> {
        self.packet_send.try_send(packet)
    }
}
