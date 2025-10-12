use godot_tokio::AsyncRuntime;
#[cfg(feature = "gossip")]
use iroh::NodeId;
use iroh::{Endpoint, endpoint::BindError, protocol::Router};
#[cfg(feature = "gossip")]
use iroh_gossip::{
    api::{GossipReceiver, GossipSender, Message},
    proto::TopicId,
};
use rand::Rng;
use tokio::sync::broadcast::{self, error::RecvError};

#[derive(Debug, Clone)]
pub enum Command {
    Close,
    #[cfg(feature = "gossip")]
    JoinTopic(Option<String>),
}

#[derive(Debug, Clone)]
pub enum Event {
    Connected(i32),
    #[cfg(feature = "gossip")]
    JoiningTopic(String),
    #[cfg(feature = "gossip")]
    JoinedTopic {
        ticket: String,
        sender: GossipSender,
        channel: broadcast::Sender<Message>,
    },
}

#[derive(Debug, Clone)]
pub struct IrohRuntime(broadcast::Sender<Command>, broadcast::Sender<Event>);

impl IrohRuntime {
    /// Initialize broadcast channels for api commands & events, then start the async runtime for `iroh`.
    pub fn spawn() -> Self {
        let (tx_commands, _) = broadcast::channel::<Command>(32);
        let (tx_events, _) = broadcast::channel::<Event>(32);

        AsyncRuntime::spawn(iroh_runtime(tx_commands.clone(), tx_events.clone()));

        Self(tx_commands, tx_events)
    }

    /// Subscribe to the api event broadcaster.
    pub fn events_recvr(&self) -> broadcast::Receiver<Event> {
        self.1.subscribe()
    }

    pub fn push_command(&self, command: Command) {
        if let Err(e) = self.0.send(command) {
            log::error!("{e}")
        }
    }
}

async fn iroh_runtime(
    commands: broadcast::Sender<Command>,
    events: broadcast::Sender<Event>,
) -> Result<(), BindError> {
    let mut commands = commands.subscribe();
    let unique_id = rand::rng().random_range(2..i32::MAX);
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let router = Router::builder(endpoint.clone());

    #[cfg(feature = "gossip")]
    let (router, gossip) = {
        use iroh_gossip::net::Gossip;

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let router = router.accept(iroh_gossip::ALPN, gossip.clone());

        (router, gossip)
    };

    #[cfg(feature = "blobs")]
    let (router, store, blobs) = {
        use godot::{classes::Os, obj::Singleton};
        use iroh_blobs::{BlobsProtocol, store::fs::FsStore};
        use std::path::Path;

        let user_data = Os::singleton().get_user_data_dir().to_string();
        let user_data_path = Path::new(&user_data);

        let store = FsStore::load(user_data_path)
            .await
            .expect("This program does not have access rights to the User Data Dir!");
        let blobs = BlobsProtocol::new(&store, endpoint.clone(), None);
        let router = router.accept(iroh_blobs::ALPN, blobs.clone());

        (router, store, blobs)
    };

    let router = router.spawn();

    if let Err(e) = events.send(Event::Connected(unique_id)) {
        log::error!("{e}");
        return Ok(());
    }

    // Command Processors.
    let close = async || {
        if let Err(e) = router.shutdown().await {
            log::error!("{e}")
        }
    };
    #[cfg(feature = "gossip")]
    let subscribe_gossip = async |topic_id: TopicId, bootstrap: Vec<NodeId>, b32_ticket: String| {
        if let Err(e) = events.send(Event::JoiningTopic(b32_ticket.clone())) {
            return log::error!("{e}");
        }

        let (send, recv) = match gossip.subscribe(topic_id, bootstrap).await {
            Ok(topic) => topic.split(),
            Err(e) => return log::error!("{e}"),
        };

        // Spawn receiver runtime, and message return channel for this topic.
        let (tx_message, _) = broadcast::channel::<Message>(128);
        tokio::spawn(topic_runtime(recv, tx_message.clone()));

        if let Err(e) = events.send(Event::JoinedTopic {
            ticket: b32_ticket,
            sender: send,
            channel: tx_message,
        }) {
            return log::error!("{e}");
        }
    };
    #[cfg(feature = "gossip")]
    let create_topic = async || {
        use iroh_gossip::proto::TopicId;
        use serde::Serialize;

        let topic_id = TopicId::from_bytes(rand::random());
        let ticket = {
            use crate::network::tickets::GossipTicket;

            let me = router.endpoint().node_id();

            let mut fbs = flexbuffers::FlexbufferSerializer::new();
            if let Err(e) = GossipTicket::new(topic_id, vec![me]).serialize(&mut fbs) {
                return log::error!("{e}");
            }

            data_encoding::BASE32_NOPAD_NOCASE.encode(fbs.view())
        };

        subscribe_gossip(topic_id, vec![], ticket).await;
    };
    #[cfg(feature = "gossip")]
    let join_topic = async |ticket: String| {
        let (raw_ticket, new_ticket) = {
            use crate::network::tickets::GossipTicket;
            use serde::{Deserialize, Serialize};

            let decode = data_encoding::BASE32_NOPAD_NOCASE
                .decode(ticket.as_bytes())
                .expect("Provided base32 ticket shuold be a valid gossip ticket string.");
            let fbr = flexbuffers::Reader::get_root(decode.as_slice())
                .expect("Provided decoded base32 buffer should be a valid gossip ticket buffer.");
            let old_ticket = GossipTicket::deserialize(fbr).unwrap();

            // Update node id chain with our id added.
            let me = router.endpoint().node_id();
            let new_node_list = old_ticket.get_nodes().into_iter().chain([me]).collect();

            // Serialize new ticket with our node id, and encode it as base32.
            let mut fbs = flexbuffers::FlexbufferSerializer::new();
            if let Err(e) =
                GossipTicket::new(old_ticket.get_topic(), new_node_list).serialize(&mut fbs)
            {
                return log::error!("{e}");
            };
            let new_ticket = data_encoding::BASE32_NOPAD_NOCASE.encode(fbs.view());

            (old_ticket, new_ticket)
        };

        subscribe_gossip(raw_ticket.get_topic(), raw_ticket.get_nodes(), new_ticket).await;
    };

    // Runtime Callbacks.
    let process_command = async |command: Command| match command {
        Command::Close => close().await,
        Command::JoinTopic(ticket) => match ticket {
            Some(ticket) => join_topic(ticket).await,
            None => create_topic().await,
        },
    };

    // Runtime loop.
    loop {
        tokio::select! {
            result = commands.recv() => match result {
                Ok(command) => process_command(command).await,
                Err(e) => match e {
                    RecvError::Closed => break,
                    _ => continue,
                },
            }
        }
    }

    Ok(())
}

#[cfg(feature = "gossip")]
async fn topic_runtime(mut receiver: GossipReceiver, channel: broadcast::Sender<Message>) {
    use futures_lite::StreamExt;

    loop {
        if !receiver.is_joined() {
            continue;
        }

        if let Some(event) = match receiver.try_next().await {
            Ok(event) => event,
            Err(e) => {
                log::error!("{e}");
                break;
            }
        } {
            match event {
                iroh_gossip::api::Event::Received(message) => {
                    if let Err(e) = channel.send(message) {
                        log::error!("{e}")
                    }
                }
                _ => log::warn!("Unhandled iroh gossip api event:\n{:?}", event),
            }
        }
    }
}
