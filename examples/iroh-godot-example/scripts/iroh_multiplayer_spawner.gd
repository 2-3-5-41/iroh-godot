extends MultiplayerSpawner

@export var iroh_player: PackedScene

var player_list: Dictionary = {};

func _ready() -> void:
	await NetworkManager.peer.bootstrapped
	multiplayer.peer_connected.connect(spawn_player)
	multiplayer.peer_disconnected.connect(despawn_player)
	spawn_player(multiplayer.get_unique_id()) # Spawn our player controler/character.

func spawn_player(id: int) -> void:
	var player: Node = iroh_player.instantiate()
	player.name = str(id)
	
	get_node(spawn_path).call_deferred("add_child", player)
	player_list[id] = player

func despawn_player(id: int) -> void:
	var player: Node = player_list[id];
	player.queue_free()
