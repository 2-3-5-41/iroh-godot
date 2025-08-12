extends Node3D

@rpc("any_peer", "call_local")
func get_caller_id():
	var caller_id = multiplayer.get_remote_sender_id()
	print(caller_id)


func _on_server_button_pressed() -> void:
	multiplayer.multiplayer_peer = IrohMultiplayerPeer.initialize_server()


func _on_client_button_pressed() -> void:
	multiplayer.multiplayer_peer = IrohMultiplayerPeer.initialize_client()
