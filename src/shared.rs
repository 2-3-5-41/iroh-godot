use iroh::endpoint::Connection;

pub type Packet = Vec<u8>;
pub type PeerId = i32;
pub type NewConnection = (Connection, PeerId);
pub type RecvPacket = (PeerId, Packet);
