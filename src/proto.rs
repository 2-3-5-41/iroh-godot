use crate::{api::ApiEvents, proto::connecting::root_as_connecting};
use godot_tokio::AsyncRuntime;
use iroh::{
    Endpoint, NodeAddr, NodeId,
    endpoint::{ConnectError, Connection, RecvStream, SendStream},
    protocol::ProtocolHandler,
};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, mpsc},
    task::JoinHandle,
};

pub const ALPN: &[u8] = b"iroh/godoto/0";
pub const MAX_PACKET_SIZE: usize = 1024;
const MAX_FLATBUFFER_SIZE: usize = 32;

#[path = "./flatbuffers/connecting_generated.rs"]
mod connecting;

enum ProtoCommand {
    PushPacket(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct MultiplayerProto {
    unique: i32,
    endpoint: Endpoint,
    tx_events: mpsc::Sender<ApiEvents>,
    connections: Arc<Mutex<HashMap<i32, MultiplayerConnection>>>,
}

impl MultiplayerProto {
    pub fn new(endpoint: Endpoint, tx_events: mpsc::Sender<ApiEvents>, unique: i32) -> Self {
        Self {
            unique,
            endpoint,
            tx_events,
            connections: Default::default(),
        }
    }
    pub fn join_peer(&self, node_addr: impl Into<NodeAddr> + Send + 'static) {
        let endpoint = self.endpoint.clone();
        let conn_map = self.connections.clone();
        let tx_events = self.tx_events.clone();

        let _: JoinHandle<Result<(), ConnectError>> = AsyncRuntime::spawn(async move {
            let mut conn_map = conn_map.lock().await;
            let tx_events = tx_events;
            let endpoint = endpoint;

            let connection = endpoint.connect(node_addr, ALPN).await?;

            let (mut send, mut recv) = connection.open_bi().await?;

            let unique = fastrand::i32(2..i32::MAX);

            let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(MAX_FLATBUFFER_SIZE);

            let connecting = connecting::Connecting::create(
                &mut fbb,
                &connecting::ConnectingArgs {
                    id: unique,
                    request_nodes: true,
                },
            );

            fbb.finish(connecting, None);

            if let Err(err) = send.write_all(fbb.finished_data()).await {
                log::error!("{err}")
            };

            let remote_id = match recv.read_i32().await {
                Ok(id) => id,
                Err(err) => {
                    log::error!("{err}");
                    -1
                }
            };

            // Receive all connections from remote.
            loop {
                let buf: &mut [u8; 32] = &mut [0u8; 32];

                match recv.read(buf).await {
                    Ok(opt) => match opt {
                        Some(size) => {
                            if size < 32 {
                                break;
                            }
                        }
                        None => break,
                    },
                    Err(err) => break log::error!("{err}"),
                }

                let node_addr = NodeId::from_bytes(buf)
                    .expect("Remote should've sent a valid NodeId byte buffer");

                let connection = endpoint.connect(node_addr, ALPN).await?;

                let (mut send, mut recv) = connection.open_bi().await?;

                let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(8);

                let connecting = connecting::Connecting::create(
                    &mut fbb,
                    &connecting::ConnectingArgs {
                        id: unique,
                        request_nodes: false,
                    },
                );

                fbb.finish(connecting, None);

                if let Err(err) = send.write_all(fbb.finished_data()).await {
                    log::error!("{err}")
                };

                let remote_id = match recv.read_i32().await {
                    Ok(id) => id,
                    Err(err) => {
                        log::error!("{err}");
                        -1
                    }
                };

                let multiplayer_connection = MultiplayerConnection::new(
                    remote_id,
                    connection,
                    (send, recv),
                    tx_events.clone(),
                );

                conn_map.insert(remote_id, multiplayer_connection);
                if let Err(err) = tx_events.send(ApiEvents::NewConnection(remote_id)).await {
                    log::error!("{err}")
                };
            }

            let multiplayer_connection =
                MultiplayerConnection::new(remote_id, connection, (send, recv), tx_events.clone());

            conn_map.insert(remote_id, multiplayer_connection);

            if let Err(err) = tx_events.send(ApiEvents::NewConnection(remote_id)).await {
                log::error!("{err}")
            };

            Ok(())
        });
    }
    pub fn push_packet(&self, id: i32, packet: Vec<u8>) {
        let conn_map = self.connections.clone();

        AsyncRuntime::spawn(async move {
            let conn_map = conn_map.lock().await;

            match conn_map.get(&id) {
                Some(connection) => {
                    connection
                        .push_command(ProtoCommand::PushPacket(packet))
                        .await
                }
                None => return,
            }
        });
    }
    pub fn disconnect_node(&self, unique: i32) {
        let conn_map = self.connections.clone();

        AsyncRuntime::spawn(async move {
            let mut conn_map = conn_map.lock().await;

            if let Some(connection) = conn_map.remove(&unique) {
                connection.close();
            }
        });
    }
}

impl ProtocolHandler for MultiplayerProto {
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        let mut conn_map = self.connections.lock().await;

        let (mut send, mut recv) = connection.accept_bi().await?;

        let buf: &mut [u8; MAX_FLATBUFFER_SIZE] = &mut [0u8; MAX_FLATBUFFER_SIZE];

        if let Err(err) = recv.read(buf).await {
            log::error!("{err}")
        };

        let connecting =
            root_as_connecting(buf).expect("Remote peer should send a valid flatbuffer");

        // Send our unique id to remote peer.
        send.write_i32(self.unique).await?;

        if connecting.request_nodes() {
            // Send all node ids of connections we have to remote peer.
            for (_, connection) in conn_map.iter() {
                if let Err(err) = send.write_all(connection.node_id().as_bytes()).await {
                    log::error!("{err}")
                };
            }

            // Send a zero byte to indicate the end of this tarnsmission of nodes to connect to.
            if let Err(err) = send.write_all(&[0u8; 1]).await {
                log::error!("{err}")
            };
        }

        let multiplayer_connection = MultiplayerConnection::new(
            connecting.id(),
            connection,
            (send, recv),
            self.tx_events.clone(),
        );

        conn_map.insert(connecting.id(), multiplayer_connection);

        if let Err(err) = self
            .tx_events
            .send(ApiEvents::NewConnection(connecting.id()))
            .await
        {
            log::error!("{err}")
        };

        Ok(())
    }

    async fn shutdown(&self) {
        let mut conn_map = self.connections.lock().await;
        conn_map
            .drain()
            .for_each(|(_, connection)| connection.close());
    }
}

#[derive(Debug)]
struct MultiplayerConnection {
    connection: Connection,
    tx_commands: mpsc::Sender<ProtoCommand>,
}

impl MultiplayerConnection {
    fn new(
        unique: i32,
        connection: Connection,
        stream: (SendStream, RecvStream),
        tx_events: mpsc::Sender<ApiEvents>,
    ) -> Self {
        let (tx_commands, rx_commands) = mpsc::channel::<ProtoCommand>(32);

        // Spawn connection runtime.
        AsyncRuntime::spawn(async move {
            let unique = unique;
            let events = tx_events;
            let mut commands = rx_commands;
            let (mut send, mut recv) = stream;

            let buf: &mut [u8; MAX_PACKET_SIZE] = &mut [0u8; MAX_PACKET_SIZE];

            loop {
                tokio::select! {
                    command = commands.recv() => match command {
                        Some(command) => match command {
                            ProtoCommand::PushPacket(packet) => if let Err(err) = send.write_all(&packet).await {
                                log::error!("{err}")
                            },
                        },
                        None => break,
                    },
                    read = recv.read(buf) => {
                        if let Err(err) = read {
                            break log::error!("{err}");
                        }

                        if let Err(err) = events.send(ApiEvents::RecvPacket((unique, buf.to_vec()))).await {
                            break log::error!("{err}")
                        };
                    }
                }
            }
        });
        Self {
            connection,
            tx_commands,
        }
    }
    fn node_id(&self) -> NodeId {
        self.connection
            .remote_node_id()
            .expect("Remote connection should maintain a valid NodeId")
    }
    async fn push_command(&self, command: ProtoCommand) {
        if let Err(err) = self.tx_commands.send(command).await {
            log::error!("{err}")
        }
    }
    fn close(&self) {
        self.connection
            .close(0u8.into(), b"Connection closed via request from remote");
    }
}
