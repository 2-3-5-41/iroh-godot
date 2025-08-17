extends Node3D

@export var is_server_reader: Label
@export var peer_id_reader: Label
@export var connect_id_input: TextEdit
@export var local_node_id_reader: TextEdit

var peer: IrohMultiplayerPeer

func _ready() -> void:
	multiplayer.multiplayer_peer = IrohMultiplayerPeer.initialize()
	if multiplayer.multiplayer_peer is IrohMultiplayerPeer:
		peer = multiplayer.multiplayer_peer
	
	# Lets pass our local node id to our text field so we can copy
	# and share it to another client.
	local_node_id_reader.text = peer.node_id()
	
	multiplayer.peer_connected.connect(_on_peer_connected)
	multiplayer.peer_disconnected.connect(_on_peer_disconnected)
	peer.connected_to_peer.connect(_on_connected_to_peer)
	peer.connecting_to_peer.connect(_on_connecting_to_peer)

func _process(_delta: float) -> void:
	if multiplayer.is_server():
		is_server_reader.text = "Is server: TRUE"
	else :
		is_server_reader.text = "Is server: FALSE"
	
	peer_id_reader.text = str(multiplayer.get_unique_id())

func _on_connecting_to_peer(remote: String) -> void:
	print("[",peer.node_id(),"] ","Connecting to peer: ",remote)

func _on_peer_connected(id: int) -> void:
	print("[",peer.node_id(),"] ",id, " Connected")

func _on_connected_to_peer(id: int) -> void:
	print("[",peer.node_id(),"] "," Connected to peer, and was assined id: ", id)

func _on_peer_disconnected(id: int) -> void:
	print("[",peer.node_id(),"] ",id, " Disconnected")

func _on_connect_pressed() -> void:
	var node_id: String = connect_id_input.text
	peer.join(node_id)

func _on_broadcast_pressed() -> void:
	_broadcast_local_id.rpc_id(1, peer.node_id())

@rpc("any_peer", "call_local", "reliable")
func _broadcast_local_id(node_id: String) -> void:
	var remote_sender = multiplayer.get_remote_sender_id()
	print("[",peer.node_id(),"] ","Got iroh NodeId: ", node_id, "\n	From: ", remote_sender)
