extends Node3D

@export var connect_id_input: TextEdit
@export var local_node_id_reader: TextEdit

var peer = IrohMultiplayerPeer.initialize()

func _ready() -> void:
	multiplayer.multiplayer_peer = peer
	
	# Lets pass our local node id to our text field so we can copy
	# and share it to another client.
	local_node_id_reader.text = peer.node_id()
	
	multiplayer.peer_connected.connect(_on_peer_connected)
	multiplayer.peer_disconnected.connect(_on_peer_disconnected)
	peer.connecting_to_peer.connect(_on_connecting_to_peer)

func _on_connecting_to_peer(remote: String) -> void:
	print("[",peer.node_id(),"] ","Connecting to peer: ",remote)

func _on_peer_connected(id: int) -> void:
	print("[",peer.node_id(),"] ",id, " Connected\n")
	_broadcast_local_id.rpc(id, peer.node_id())

func _on_peer_disconnected(id: int) -> void:
	print("[",peer.node_id(),"] ",id, " Disconnected\n")

func _on_connect_pressed() -> void:
	var node_id: String = connect_id_input.text
	peer.join(node_id)


@rpc("any_peer")
func _broadcast_local_id(node_id: String) -> void:
	var remote_sender = multiplayer.get_remote_sender_id()
	print("[",peer.node_id(),"] ","Got iroh NodeId: ", node_id, "\n	From: ", remote_sender)
