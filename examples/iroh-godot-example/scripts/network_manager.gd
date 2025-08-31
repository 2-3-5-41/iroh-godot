extends Node

const PORT: int = 23541

var peer: IrohMultiplayerPeer

func _ready() -> void:
	peer = IrohMultiplayerPeer.new()
	multiplayer.multiplayer_peer = peer

func connect_to_node(node: String) :
	peer.join(node)

func node_id() -> String:
	return peer.node_id()
