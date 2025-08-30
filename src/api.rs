use godot_tokio::AsyncRuntime;
use iroh::{Endpoint, NodeAddr, NodeId, protocol::Router};
use tokio::sync::mpsc;

use crate::proto::{ALPN, MultiplayerProto};

#[derive(Debug)]
pub enum ApiEvents {
    Bind { unique_id: i32, node_id: NodeId },
    NewConnection(i32),
    RecvPacket((i32, Vec<u8>)),
}

#[derive(Debug)]
pub enum ApiCommands {
    JoinNode(NodeAddr),
    PushPacket { id: i32, packet: Vec<u8> },
    BroadcastPacket(Vec<u8>),
    DisconnectNode(i32),
    Close,
}

pub struct MultiplayerApi {
    rx_events: mpsc::Receiver<ApiEvents>,
    tx_commands: mpsc::Sender<ApiCommands>,
}

impl MultiplayerApi {
    pub fn spawn() -> Self {
        let (tx_events, rx_events) = mpsc::channel::<ApiEvents>(64);
        let (tx_commands, rx_commands) = mpsc::channel::<ApiCommands>(64);
        AsyncRuntime::spawn(start(tx_events, rx_commands));
        Self {
            rx_events,
            tx_commands,
        }
    }
    pub fn recv_event(&mut self) -> Result<ApiEvents, mpsc::error::TryRecvError> {
        self.rx_events.try_recv()
    }
    pub fn push_command(&self, command: ApiCommands) {
        let tx_commands = self.tx_commands.clone();
        AsyncRuntime::spawn(async move {
            if let Err(err) = tx_commands.send(command).await {
                log::error!("{err}")
            }
        });
    }
}

async fn start(tx_events: mpsc::Sender<ApiEvents>, mut rx_commands: mpsc::Receiver<ApiCommands>) {
    let endpoint = match Endpoint::builder().discovery_n0().bind().await {
        Ok(bind) => bind,
        Err(err) => return log::error!("{err}"),
    };

    let multiplayer_proto = MultiplayerProto::new(endpoint.clone(), tx_events.clone());

    let router = Router::builder(endpoint.clone())
        .accept(ALPN, multiplayer_proto.clone())
        .spawn();

    if let Err(err) = tx_events
        .send(ApiEvents::Bind {
            unique_id: multiplayer_proto.unique_id(),
            node_id: endpoint.node_id(),
        })
        .await
    {
        log::error!("{err}")
    };

    loop {
        let command = match rx_commands.recv().await {
            Some(command) => command,
            None => break,
        };

        match command {
            ApiCommands::JoinNode(node_addr) => multiplayer_proto.join_peer(node_addr),
            ApiCommands::PushPacket { id, packet } => multiplayer_proto.push_packet(id, packet),
            ApiCommands::BroadcastPacket(packet) => multiplayer_proto.broadcast_packet(packet),
            ApiCommands::DisconnectNode(id) => multiplayer_proto.disconnect_node(id),
            ApiCommands::Close => {
                if let Err(err) = router.shutdown().await {
                    log::error!("{err}")
                }
            }
        }
    }
}
