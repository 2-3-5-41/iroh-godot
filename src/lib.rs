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
    endpoint: Arc<Endpoint>,
    connection_recv_channel: tokio::sync::mpsc::Receiver<IrohConnection>,
    connections: HashMap<PeerId, IrohConnection>,
    packets: VecDeque<(PeerId, Vec<u8>)>,
    target_peer: PeerId,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    // Required methods.
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
        true
    }
    fn poll(&mut self) {
        // Receive and store iroh connections.
        while let Ok(conn) = self.connection_recv_channel.try_recv() {
            let peer_id = fastrand::i32(1..i32::MAX);
            self.connections.insert(peer_id, conn);
            self.base_mut()
                .signals()
                .peer_connected()
                .emit(peer_id as i64);
        }

        // Receive packets from all connecitons.
        self.connections.iter_mut().for_each(|(id, conn)| {
            while let Some(packet) = conn.recv_packet() {
                self.packets.push_back((*id, packet));
            }
        });
    }
    fn close(&mut self) {
        AsyncRuntime::block_on(self.endpoint.clone().close())
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        if let Some(conn) = self.connections.get(&p_peer) {
            conn.get_connection()
                .close(VarInt::from_u32(0), b"Disconnect Request");
        }
        self.base_mut()
            .signals()
            .peer_disconnected()
            .emit(p_peer as i64);
    }
    fn get_unique_id(&self) -> i32 {
        -1i32
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        if self.endpoint.is_closed() {
            ConnectionStatus::DISCONNECTED
        } else {
            ConnectionStatus::CONNECTED
        }
    }

    // Optional methods.
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        self.connections.iter().for_each(|(_, peer)| {
            peer.send_packet(p_buffer.to_vec());
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
    fn connect() -> Gd<Self> {
        let _ = AsyncRuntime::runtime().enter();

        let endpoint = Arc::new(
            AsyncRuntime::block_on(
                Endpoint::builder()
                    .discovery_n0()
                    .alpns(vec![ALPN.to_vec()])
                    .bind(),
            )
            .expect("Iroh Endpoint"),
        );

        let (conn_send, conn_recv) = tokio::sync::mpsc::channel::<IrohConnection>(128);

        let endpoint_clone = endpoint.clone();
        AsyncRuntime::spawn(async move {
            let endpoint = endpoint_clone;
            let conn_send = conn_send;

            while let Some(incoming) = endpoint.accept().await {
                let connecting = match incoming.accept() {
                    Ok(connection) => connection,
                    Err(err) => {
                        eprintln!("Incoming connection failed: {}", err);
                        continue;
                    }
                };

                let connection = match connecting.await {
                    Ok(conn) => IrohConnection::new(conn).await,
                    Err(err) => {
                        eprintln!("Connection error: {}", err);
                        continue;
                    }
                };

                if let Err(err) = conn_send.send(connection).await {
                    eprintln!("Connection sync channel send error: {}", err);
                }
            }
        });

        Gd::from_init_fn(|base| Self {
            base,
            endpoint,
            connection_recv_channel: conn_recv,
            connections: HashMap::new(),
            packets: VecDeque::new(),
            target_peer: 0,
        })
    }
}

struct IrohConnection {
    connection: Connection,
    packet_recv: tokio::sync::mpsc::Receiver<Packet>,
    packet_send: tokio::sync::mpsc::Sender<Packet>,
}

impl IrohConnection {
    async fn new(connection: Connection) -> Self {
        // Receive uni-directional packets loop.
        let (in_packet_send, in_packet_recv) = tokio::sync::mpsc::channel::<Packet>(128);
        let conn_clone = connection.clone();
        AsyncRuntime::spawn(async move {
            let mut s_recv = conn_clone
                .accept_uni()
                .await
                .expect("Uni-directional stream");

            let in_channel = in_packet_send;

            loop {
                let buf: &mut [u8; MAX_ALLOWED_PACKET_SIZE] = &mut [0u8; MAX_ALLOWED_PACKET_SIZE];
                match s_recv.read_exact(buf).await {
                    Ok(_) => {
                        if let Err(err) = in_channel.send(buf.to_vec()).await {
                            eprintln!("Send Error: {}", err)
                        }
                    }
                    Err(err) => {
                        eprintln!("Read Exact Error: {}", err);
                        continue;
                    }
                }
            }
        });

        // Send uni-directional packets loop.
        let (out_packet_send, out_packet_recv) = tokio::sync::mpsc::channel::<Packet>(128);
        let conn_clone = connection.clone();
        AsyncRuntime::spawn(async move {
            let mut s_send = conn_clone.open_uni().await.expect("Uni-directional stream");

            let mut out_channel = out_packet_recv;

            loop {
                if let Some(packet) = out_channel.recv().await {
                    if let Err(err) = s_send.write_all(packet.as_slice()).await {
                        eprintln!("{}", err);
                    }
                } else {
                    continue;
                }
            }
        });

        Self {
            connection,
            packet_recv: in_packet_recv,
            packet_send: out_packet_send,
        }
    }

    fn get_connection(&self) -> &Connection {
        &self.connection
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

    fn send_packet(&self, packet: Packet) {
        if let Err(err) = self.packet_send.try_send(packet) {
            eprintln!("{}", err);
        }
    }
}
