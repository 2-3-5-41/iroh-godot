use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

use godot_tokio::AsyncRuntime;
use iroh::{Endpoint, endpoint::BindError, protocol::Router};

use crate::protocol::behaviour::GodotMultiplayerProto;

pub struct Peer {
    inner: Router,
    multiplayer_proto: GodotMultiplayerProto,
}

impl Peer {
    pub fn init(port: u16) -> Result<Self, BindError> {
        AsyncRuntime::block_on(async move {
            let endpoint = Endpoint::builder()
                .discovery_n0()
                .bind_addr_v4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
                .bind_addr_v6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, port, 0, 0))
                .bind()
                .await?;

            let multiplayer_proto = GodotMultiplayerProto::new(endpoint.clone());

            let router = Router::builder(endpoint)
                .accept(crate::protocol::ALPN, multiplayer_proto.clone())
                .spawn();

            Ok(Self {
                inner: router,
                multiplayer_proto,
            })
        })
    }
    pub fn router(&self) -> &Router {
        &self.inner
    }
    pub fn endpoint(&self) -> &Endpoint {
        &self.router().endpoint()
    }
    pub fn subscribe_inbound_connections(
        &self,
    ) -> tokio::sync::broadcast::Receiver<(i32, crate::protocol::connections::RemoteConnection)>
    {
        self.multiplayer_proto.subscribe_inbound_connections()
    }
    pub fn get_unique_id(&self) -> i32 {
        self.multiplayer_proto.get_unique_id()
    }
    pub fn multiplayer(&self) -> &GodotMultiplayerProto {
        &self.multiplayer_proto
    }
}
