#[cfg(feature = "gossip")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "gossip")]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GossipTicket {
    topic: iroh_gossip::proto::TopicId,
    nodes: Vec<iroh::NodeId>,
}

#[cfg(feature = "gossip")]
impl GossipTicket {
    pub fn new(topic: iroh_gossip::proto::TopicId, nodes: Vec<iroh::NodeId>) -> Self {
        Self { topic, nodes }
    }
    pub fn get_topic(&self) -> iroh_gossip::proto::TopicId {
        self.topic.clone()
    }
    pub fn get_nodes(&self) -> Vec<iroh::NodeId> {
        self.nodes.clone()
    }
}
