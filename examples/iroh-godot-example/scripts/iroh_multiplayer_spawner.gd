extends MultiplayerSpawner

@export var iroh_player: PackedScene

func _ready() -> void:
	multiplayer.peer_connected.connect(spawn_player) # Spawn a player for the remote peers.
	spawn_player(multiplayer.get_unique_id()) # Spawn our player for us.

func spawn_player(id: int) -> void:
	var player: Node = iroh_player.instantiate()
	player.name = str(id)
	
	get_node(spawn_path).call_deferred("add_child", player)
