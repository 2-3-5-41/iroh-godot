extends PanelContainer

@export var public_id_text: TextEdit

func _process(_delta: float) -> void:
	if multiplayer.multiplayer_peer is IrohMultiplayerPeer:
		public_id_text.text = multiplayer.multiplayer_peer.get_public_id()
