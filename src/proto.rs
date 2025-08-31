use crate::{
    api::ApiEvents,
    proto::{
        accepting::{Accepting, AcceptingArgs, NodeIdent, root_as_accepting},
        connecting::root_as_connecting,
    },
};
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
pub const MAX_PACKET_SIZE: usize = 2usize.pow(8);
const CONNECT_BUFFER_SIZE: usize = 2usize.pow(5);
const ACCEPT_BUFFER_SIZE: usize = 2usize.pow(12);

#[path = "./flatbuffers/accepting_generated.rs"]
mod accepting;
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
    pub fn new(endpoint: Endpoint, tx_events: mpsc::Sender<ApiEvents>) -> Self {
        let unique = fastrand::i32(2..i32::MAX);
        Self {
            unique,
            endpoint,
            tx_events,
            connections: Default::default(),
        }
    }
    pub fn unique_id(&self) -> i32 {
        self.unique
    }
    pub fn join_peer(&self, node_addr: impl Into<NodeAddr> + Send + 'static) {
        let endpoint = self.endpoint.clone();
        let conn_map = self.connections.clone();
        let tx_events = self.tx_events.clone();
        let unique = self.unique;

        let _: JoinHandle<Result<(), ConnectError>> = AsyncRuntime::spawn(async move {
            let mut conn_map = conn_map.lock().await;
            let tx_events = tx_events;
            let endpoint = endpoint;
            let unique = unique;

            // Connect to node
            let connection = endpoint.connect(node_addr, ALPN).await?;

            // Open bi-directional stream
            let (mut send, mut recv) = connection.open_bi().await?;

            // Create flatbuffer to share our ID and request for other connected nodes.
            let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(CONNECT_BUFFER_SIZE);

            let connecting = connecting::Connecting::create(
                &mut fbb,
                &connecting::ConnectingArgs {
                    id: unique,
                    request_nodes: true,
                },
            );

            fbb.finish(connecting, None);

            // Send flatbuffer to remote.
            if let Err(err) = send.write_all(fbb.finished_data()).await {
                log::error!("{err}")
            };

            // Receive remote's accepting flatbuffer
            let buf: &mut [u8; ACCEPT_BUFFER_SIZE] = &mut [0u8; ACCEPT_BUFFER_SIZE];

            if let Err(err) = recv.read(buf).await {
                log::error!("{err}")
            }

            match root_as_accepting(buf) {
                Ok(accepting) => {
                    let remote_id = accepting.id();
                    let connections = accepting.nodes();

                    match connections {
                        Some(vec) => {
                            for ident in vec {
                                let node_addr = NodeId::from_bytes(&ident.0)
                                    .expect("Remote should provide valid NodeId");

                                // Connect to node
                                let connection = endpoint.connect(node_addr, ALPN).await?;

                                // Open bi-directional stream
                                let (mut send, mut recv) = connection.open_bi().await?;

                                // Create flatbuffer to share our ID and request for other connected nodes.
                                let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(
                                    CONNECT_BUFFER_SIZE,
                                );

                                let connecting = connecting::Connecting::create(
                                    &mut fbb,
                                    &connecting::ConnectingArgs {
                                        id: unique,
                                        request_nodes: false,
                                    },
                                );

                                fbb.finish(connecting, None);

                                // Send flatbuffer to remote.
                                if let Err(err) = send.write_all(fbb.finished_data()).await {
                                    log::error!("{err}")
                                };

                                // Receive remote's accepting flatbuffer
                                let buf: &mut [u8; ACCEPT_BUFFER_SIZE] =
                                    &mut [0u8; ACCEPT_BUFFER_SIZE];

                                if let Err(err) = recv.read(buf).await {
                                    log::error!("{err}")
                                }

                                match root_as_accepting(buf) {
                                    Ok(accepting) => {
                                        let remote_id = accepting.id();

                                        // Create connection object
                                        let multiplayer_connection = MultiplayerConnection::new(
                                            remote_id,
                                            connection,
                                            (send, recv),
                                            tx_events.clone(),
                                        );

                                        // Store new connection object
                                        conn_map.insert(remote_id, multiplayer_connection);

                                        // Notify main thread of new connection
                                        if let Err(err) = tx_events
                                            .send(ApiEvents::NewConnection(remote_id))
                                            .await
                                        {
                                            log::error!("{err}")
                                        };
                                    }
                                    Err(err) => log::error!("{err}"),
                                }
                            }

                            // Create connection object
                            let multiplayer_connection = MultiplayerConnection::new(
                                remote_id,
                                connection,
                                (send, recv),
                                tx_events.clone(),
                            );

                            // Store new connection object
                            conn_map.insert(remote_id, multiplayer_connection);

                            // Notify main thread of new connection
                            if let Err(err) =
                                tx_events.send(ApiEvents::NewConnection(remote_id)).await
                            {
                                log::error!("{err}")
                            };
                        }
                        None => log::info!("No one else to connect to"),
                    }
                }
                Err(err) => log::error!("{err}"),
            }

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
    pub fn broadcast_packet(&self, packet: Vec<u8>) {
        let conn_map = self.connections.clone();

        AsyncRuntime::spawn(async move {
            let conn_map = conn_map.lock().await;
            let packet = packet;

            for (_, connection) in conn_map.iter() {
                connection
                    .push_command(ProtoCommand::PushPacket(packet.clone()))
                    .await;
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

        // Accept bi-directional stream
        let (mut send, mut recv) = connection.accept_bi().await?;

        let buf: &mut [u8; CONNECT_BUFFER_SIZE] = &mut [0u8; CONNECT_BUFFER_SIZE];

        // Read flatbuffer packet into buffer
        if let Err(err) = recv.read(buf).await {
            log::error!("{err}")
        };

        // Create root object from flatbuffer
        let connecting =
            root_as_connecting(buf).expect("Remote peer should send a valid flatbuffer");

        // Create our return flatbuffer
        let mut fbb = flatbuffers::FlatBufferBuilder::with_capacity(ACCEPT_BUFFER_SIZE);

        let nodes = match connecting.request_nodes() {
            true => {
                fbb.start_vector::<NodeIdent>(conn_map.len());
                conn_map.iter().for_each(|(_, connection)| {
                    let ident = NodeIdent::new(connection.node_id().as_bytes());
                    fbb.push(ident);
                });

                Some(fbb.end_vector::<NodeIdent>(conn_map.len()))
            }
            false => None,
        };

        let accepting = Accepting::create(
            &mut fbb,
            &AcceptingArgs {
                id: self.unique,
                nodes,
            },
        );

        fbb.finish(accepting, None);

        // Send our accepting flatbuffer to remote
        if let Err(err) = send.write_all(fbb.finished_data()).await {
            log::error!("{err}")
        };

        // Create new connection object from accepted peer's details
        let multiplayer_connection = MultiplayerConnection::new(
            connecting.id(),
            connection,
            (send, recv),
            self.tx_events.clone(),
        );

        // Store new connection object
        conn_map.insert(connecting.id(), multiplayer_connection);

        // Notify main thread of new connection
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

            loop {
                tokio::select! {
                    command = commands.recv() => match command {
                        Some(command) => match command {
                            ProtoCommand::PushPacket(packet) => {
                                let packet_len = packet.len();
                                if let Err(err) = send.write_u16(packet_len as u16).await {
                                    log::error!("{err}")
                                };
                                if let Err(err) = send.write_all(packet.as_slice()).await {
                                    log::error!("{err}")
                                }
                            }
                        },
                        None => break log::warn!("Protocol Commands Channel Closed"),
                    },
                    read_size = recv.read_u16() => match read_size {
                        Ok(size) => {
                            let mut buf = vec![0u8; size as usize];

                            if let Err(err) = recv.read(&mut buf).await {
                                log::error!("{err}")
                            };

                            if let Err(err) = events.send(ApiEvents::RecvPacket((unique, buf))).await {
                                log::error!("{err}")
                            };
                        },
                        Err(err) => log::error!("{err}"),
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
