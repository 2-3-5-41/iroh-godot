use crate::network::runtime::{Command, Event, IrohRuntime};
use godot::{
    classes::{
        IMultiplayerPeerExtension, MultiplayerPeerExtension,
        multiplayer_peer::{ConnectionStatus, TransferMode},
    },
    prelude::*,
};
use tokio::sync::broadcast;

#[derive(Debug, GodotClass)]
#[class(base = MultiplayerPeerExtension, tool, no_init)]
pub struct IrohMultiplayerPeer {
    base: Base<MultiplayerPeerExtension>,
    inner: IrohRuntime,
    inner_events: broadcast::Receiver<Event>,
    status: ConnectionStatus,
    multiplayer_id: i32,
}

#[godot_api]
impl IMultiplayerPeerExtension for IrohMultiplayerPeer {
    // Required methods
    fn get_available_packet_count(&self) -> i32 {
        todo!()
    }
    fn get_max_packet_size(&self) -> i32 {
        todo!()
    }
    fn get_packet_channel(&self) -> i32 {
        todo!()
    }
    fn get_packet_mode(&self) -> TransferMode {
        todo!()
    }
    fn set_transfer_channel(&mut self, p_channel: i32) {
        todo!()
    }
    fn get_transfer_channel(&self) -> i32 {
        todo!()
    }
    fn set_transfer_mode(&mut self, p_mode: TransferMode) {
        todo!()
    }
    fn get_transfer_mode(&self) -> TransferMode {
        todo!()
    }
    fn set_target_peer(&mut self, p_peer: i32) {
        todo!()
    }
    fn get_packet_peer(&self) -> i32 {
        todo!()
    }
    fn is_server(&self) -> bool {
        false
    }
    fn poll(&mut self) {
        while let Ok(event) = self.inner_events.try_recv() {
            match event {
                Event::Connected(unique_id) => {
                    self.status = ConnectionStatus::CONNECTED;
                    self.multiplayer_id = unique_id;
                    self.signals().iroh_connected().emit();
                    godot_print!("Connection established to the iroh network.")
                }
                Event::JoiningTopic(ticket) => godot_print!("Joining topic with ticket: {ticket}"),
                Event::JoinedTopic {
                    ticket,
                    sender,
                    channel,
                } => {
                    self.signals().joined_gossip_topic().emit();
                    godot_print!("Joined topic with ticket: {ticket}")
                }
            }
        }
    }
    fn close(&mut self) {
        self.inner.push_command(Command::Close);
    }
    fn disconnect_peer(&mut self, p_peer: i32, p_force: bool) {
        todo!()
    }
    fn get_unique_id(&self) -> i32 {
        self.multiplayer_id
    }
    fn get_connection_status(&self) -> ConnectionStatus {
        self.status
    }
}

#[godot_api]
impl IrohMultiplayerPeer {
    /// Emitted when the `iroh` router has spawned after establishing all protocols
    /// and a connection to a home relay server.
    #[signal]
    fn iroh_connected();
    /// Emitted when a Gossip Topic is joined successfully.
    ///
    /// Emits a Godot resource object that allows for interacting with the Gossip Topic.
    #[cfg(feature = "gossip")]
    #[signal]
    fn joined_gossip_topic();
    #[func]
    fn initialize() -> Gd<Self> {
        let inner = IrohRuntime::spawn();
        let inner_events = inner.events_recvr();
        Gd::from_init_fn(|base| Self {
            base,
            inner,
            inner_events,
            status: ConnectionStatus::CONNECTING,
            multiplayer_id: 0,
        })
    }
    /// Join a new topic, or join an existing topic with a base32 ticket.
    #[func]
    fn join_topic(&self, topic: String) {
        if topic.is_empty() {
            self.inner.push_command(Command::JoinTopic(None));
        } else {
            self.inner.push_command(Command::JoinTopic(Some(topic)));
        }
    }
}

impl IrohMultiplayerPeer {}
