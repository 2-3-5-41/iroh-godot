extends Node2D

var initialized: bool = false

func create_game():
	if initialized:
		print("Game already created...")
		return
	
	var peer = IrohMultiplayerPeer.new()
	peer.create_instance()
	await peer.public_id_changed
	multiplayer.multiplayer_peer = peer
	initialized = true


func _create_instance_pressed() -> void:
	create_game()
