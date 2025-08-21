use super::peer::Peer;
use crate::{
    godot_peer_data_generated::root_as_multiplayer_data_packet,
    protocol::connections::{MAX_PACKET_SIZE, RemoteConnection},
};
use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeer, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    global::Error,
    prelude::*,
};
use godot_tokio::AsyncRuntime;
use iroh::{NodeId, endpoint::VarInt, node_info::NodeIdExt};
use std::{
    collections::{HashMap, VecDeque},
    slice,
};
use tokio::sync::broadcast;

#[derive(GodotClass)]
#[class(base=MultiplayerPeerExtension, no_init, tool)]
pub struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    peer: Option<Peer>,
    // Connections
    inbound_connections: Option<broadcast::Receiver<(i32, RemoteConnection)>>,
    connections: HashMap<i32, RemoteConnection>,
    // Packets
    inbound_packets: HashMap<i32, broadcast::Receiver<Vec<u8>>>,
    packet_queue: VecDeque<(i32, Vec<u8>)>,
    // Targets
    target_peer: i32,
    // Tasks
    connect_tasks: VecDeque<tokio::task::JoinHandle<anyhow::Result<i32>>>,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    fn get_available_packet_count(&self) -> i32 {
        self.packet_queue.len() as i32
    }
    fn get_max_packet_size(&self) -> i32 {
        MAX_PACKET_SIZE as i32
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
        self.target_peer = p_peer
    }
    fn get_packet_peer(&self) -> i32 {
        match self.packet_queue.front() {
            Some((sender, _)) => *sender,
            None => 0,
        }
    }
    fn is_server(&self) -> bool {
        false
    }
    fn poll(&mut self) {
        // Receive and store new connections.
        if let Some(inbound_connections) = &mut self.inbound_connections {
            match inbound_connections.try_recv() {
                Ok((remote_id, conn)) => {
                    self.connections.insert(remote_id, conn.clone());
                    self.signals().peer_connected().emit(remote_id as i64);

                    if let Some(_) = self
                        .inbound_packets
                        .insert(remote_id, conn.subscribe_inbound_packets())
                    {
                        godot_warn!("Replaced old inbound packet receiver for remote {remote_id}")
                    }
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    godot_error!("Inbound connections channel has been closed, this is a bug")
                }
                _ => (),
            }
        }

        // Process outbound connection tasks.
        if let Some(task) = self.connect_tasks.pop_front() {
            match task.is_finished() {
                true => {
                    let _ = AsyncRuntime::block_on(task);
                }
                false => self.connect_tasks.push_back(task),
            };
        }

        // Receive and process inbound packets from each connection.
        self.inbound_packets
            .iter_mut()
            .for_each(|(_, receiver)| match receiver.try_recv() {
                Ok(packet) => {
                    match process_packet(packet.as_slice()) {
                        Ok(value) => self.packet_queue.push_back(value),
                        Err(err) => godot_error!("{err}"),
                    };
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    godot_error!("Packet receiver channel closed, this is a bug");
                }
                _ => (),
            });
    }
    fn close(&mut self) {
        if let Some(peer) = &self.peer {
            AsyncRuntime::block_on(peer.endpoint().close());
        }
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        match self.connections.remove(&p_peer) {
            Some(connection) => {
                connection
                    .get_connection()
                    .close(VarInt::from_u32(0), b"Disconnected from peer by request");
            }
            None => godot_error!("There is no remote connection with peer ID {p_peer}"),
        }
    }
    fn get_unique_id(&self) -> i32 {
        if let Some(peer) = &self.peer {
            return peer.get_unique_id();
        }
        0
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        match self.peer {
            Some(_) => ConnectionStatus::CONNECTED,
            None => ConnectionStatus::DISCONNECTED,
        }
    }
    fn get_packet_script(&mut self) -> PackedByteArray {
        match self.packet_queue.pop_front() {
            Some((_, data)) => data.into(),
            None => PackedByteArray::default(),
        }
    }
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        match self.target_peer {
            MultiplayerPeer::TARGET_PEER_BROADCAST => {
                self.connections
                    .iter()
                    .for_each(|(_, connection)| connection.push_packet(p_buffer.to_vec()));
                Error::OK
            }
            _ => match self.connections.get(&self.target_peer) {
                Some(connection) => {
                    connection.push_packet(p_buffer.to_vec());
                    Error::OK
                }
                None => Error::ERR_DOES_NOT_EXIST,
            },
        }
    }
    // unsafe fn get_packet_rawptr(
    //     &mut self,
    //     r_buffer: *mut *const u8,
    //     r_buffer_size: *mut i32,
    // ) -> Error {
    //     // Sanity & saftey checks.
    //     if r_buffer.is_null() {
    //         return Error::ERR_DOES_NOT_EXIST;
    //     }
    //     if r_buffer_size.is_null() {
    //         return Error::ERR_DOES_NOT_EXIST;
    //     }

    //     match self.packet_queue.pop_front() {
    //         Some((sender, packet)) => {
    //             godot_print!("Reading packet from {}", sender);

    //             unsafe {
    //                 *r_buffer = packet.as_ptr();
    //                 *r_buffer_size = packet.len() as i32
    //             }

    //             Error::OK
    //         }
    //         None => Error::ERR_DOES_NOT_EXIST,
    //     }
    // }
    // unsafe fn put_packet_rawptr(&mut self, r_buffer: *const u8, r_buffer_size: i32) -> Error {
    //     // Sanity & saftey checks.
    //     if r_buffer.is_null() {
    //         return Error::ERR_DOES_NOT_EXIST;
    //     }

    //     // Get our Godot scene multiplayer packet as a slice.
    //     let packet: &[u8] = unsafe { slice::from_raw_parts(r_buffer, r_buffer_size as usize) };

    // match self.target_peer {
    //     MultiplayerPeer::TARGET_PEER_BROADCAST => {
    //         self.connections
    //             .iter()
    //             .for_each(|(_, connection)| connection.push_packet(packet.to_vec()));
    //         Error::OK
    //     }
    //     _ => match self.connections.get(&self.target_peer) {
    //         Some(connection) => {
    //             connection.push_packet(packet.to_vec());
    //             Error::OK
    //         }
    //         None => Error::ERR_DOES_NOT_EXIST,
    //     },
    // }
    // }
}

#[godot_api]
impl IrohMultiplayerPeer {
    #[func]
    fn bootstrap(port: u16) -> Gd<Self> {
        tracing_subscriber::fmt().init();
        let mut inbound_connections: Option<broadcast::Receiver<(i32, RemoteConnection)>> = None;
        let peer = match Peer::init(port) {
            Ok(peer) => {
                inbound_connections = Some(peer.subscribe_inbound_connections());
                Some(peer)
            }
            Err(err) => {
                godot_error!("{err}");
                None
            }
        };

        Gd::from_init_fn(|base| Self {
            base,
            peer,
            inbound_connections,
            connections: HashMap::default(),
            inbound_packets: HashMap::default(),
            packet_queue: VecDeque::default(),
            target_peer: 0,
            connect_tasks: VecDeque::default(),
        })
    }
    #[func]
    fn connect_to_node(&mut self, node: String) {
        let multiplayer = match &self.peer {
            Some(peer) => peer.multiplayer(),
            None => {
                return godot_error!(
                    "You need to be bootstrapped to the iroh network first, make sure you call `bootstrap`"
                );
            }
        };
        let node_id = match NodeId::from_z32(&node) {
            Ok(id) => id,
            Err(err) => return godot_error!("{err}"),
        };
        let task = multiplayer.connect(node_id);
        self.connect_tasks.push_back(task);
    }
    #[func]
    fn local_node_id(&self) -> String {
        match &self.peer {
            Some(peer) => peer.endpoint().node_id().to_z32(),
            None => {
                godot_error!("Make sure you bootstrap the iroh netowrk by calling `bootstrap`");
                "Not connected to the iroh network".into()
            }
        }
    }
    #[func]
    fn remote_node_id(&self, peer: i32) -> String {
        match self.connections.get(&peer) {
            Some(remote) => remote.get_node_id(),
            None => "Not a connected peer ID".into(),
        }
    }
}

fn process_packet<'a>(packet: &'a [u8]) -> Result<(i32, Vec<u8>), flatbuffers::InvalidFlatbuffer> {
    match root_as_multiplayer_data_packet(packet) {
        Ok(deserialized) => {
            let sender = deserialized.sender();
            let packet = deserialized
                .packet()
                .iter()
                .map(|byte| byte)
                .collect::<Vec<u8>>();
            Ok((sender, packet))
        }
        Err(err) => Err(err),
    }
}
