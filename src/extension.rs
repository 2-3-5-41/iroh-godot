use crate::{
    api::{ApiCommands, ApiEvents, MultiplayerApi},
    proto::MAX_PACKET_SIZE,
};
use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    global::Error,
    prelude::*,
};
use iroh::{NodeId, node_info::NodeIdExt};
use std::collections::VecDeque;
use tokio::sync::mpsc::error::TryRecvError;

#[derive(GodotClass)]
#[class(base=MultiplayerPeerExtension, tool, no_init)]
struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    api: MultiplayerApi,
    unique_id: i32,
    node_id: Option<NodeId>,
    target_peer: i32,
    status: ConnectionStatus,
    recv_packets: VecDeque<(i32, PackedByteArray)>,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    // Required methods
    fn get_available_packet_count(&self) -> i32 {
        self.recv_packets.len() as i32
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
        match self.recv_packets.front() {
            Some((id, _)) => *id,
            None => -1,
        }
    }
    /// This will always return `false` as an `iroh` peer cannot be a standalone server.
    /// If a peer needs to act as the game authority, you must set them as the multiplayer authority in your game logic.
    fn is_server(&self) -> bool {
        // It would be an error for an IrohMultiplayerPeer to be a 'server'
        false
    }
    fn poll(&mut self) {
        let event = match self.api.recv_event() {
            Ok(event) => event,
            Err(err) => {
                if err == TryRecvError::Disconnected {
                    godot_error!("{err}")
                };
                return;
            }
        };

        match event {
            ApiEvents::Bind { unique_id, node_id } => {
                self.unique_id = unique_id;
                self.node_id.replace(node_id);
                self.status = ConnectionStatus::CONNECTED;
                self.signals().bootstrapped().emit();
            }
            ApiEvents::NewConnection(unique) => {
                self.signals().peer_connected().emit(unique as i64);
            }
            ApiEvents::LostConnection(unique) => {
                self.signals().peer_disconnected().emit(unique as i64);
            }
            ApiEvents::RecvPacket((unique, packet)) => {
                self.recv_packets.push_back((unique, packet.into()));
            }
        }
    }
    fn close(&mut self) {
        self.api.push_command(ApiCommands::Close);
    }
    fn disconnect_peer(&mut self, p_peer: i32, _p_force: bool) {
        self.api.push_command(ApiCommands::DisconnectNode(p_peer));
    }
    fn get_unique_id(&self) -> i32 {
        self.unique_id
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        self.status
    }
    // Provided methods
    fn get_packet_script(&mut self) -> PackedByteArray {
        let (_, packet) = self
            .recv_packets
            .pop_front()
            .expect("There should be a packet available");
        packet
    }
    fn put_packet_script(&mut self, p_buffer: PackedByteArray) -> Error {
        match self.target_peer {
            0 => self
                .api
                .push_command(ApiCommands::BroadcastPacket(p_buffer.to_vec())),
            _ => self.api.push_command(ApiCommands::PushPacket {
                id: self.target_peer,
                packet: p_buffer.to_vec(),
            }),
        }
        Error::OK
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    /// Signal emitted once the `iroh` async runtime has been successfully established.
    #[signal]
    fn bootstrapped();

    /// Start the `iroh` async runtime, and bind our endpoint (on IPV4 & IPV6) to the provided port number.
    #[func]
    fn bootstrap(port: u16) -> Gd<Self> {
        let api = MultiplayerApi::spawn(port);
        Gd::from_init_fn(|base| Self {
            base,
            api,
            unique_id: 0,
            node_id: None,
            target_peer: 0,
            status: ConnectionStatus::CONNECTING,
            recv_packets: Default::default(),
        })
    }

    /// Request the `iroh` async runtime connect to a node via a z-base-32 NodeId (NodeAddr).
    #[func]
    fn join(&self, node_addr: String) {
        let node = match NodeId::from_z32(&node_addr) {
            Ok(node_addr) => node_addr,
            Err(err) => return godot_error!("{err}"),
        };
        self.api.push_command(ApiCommands::JoinNode(node.into()));
    }

    /// Read our local z-base-32 NodeId string.
    #[func]
    fn local_node_id(&self) -> String {
        match self.node_id {
            Some(id) => id.to_z32(),
            None => "NULL".into(),
        }
    }
}
