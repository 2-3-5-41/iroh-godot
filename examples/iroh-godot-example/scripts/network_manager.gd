extends Node

const PORT: int = 23541

var peer: IrohMultiplayerPeer

func _ready() -> void:
	peer = IrohMultiplayerPeer.bootstrap(PORT)
	multiplayer.multiplayer_peer = peer

func connect_to_node(node: String) :
	peer.join(node)

func node_id() -> String:
	return peer.local_node_id()

func disconnect_node(id: int) -> void:
	peer.disconnect_peer(id)
