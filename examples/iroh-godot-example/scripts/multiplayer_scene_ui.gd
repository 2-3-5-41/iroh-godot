extends Control

@onready var input: TextEdit = $VBoxContainer/HBoxContainer/TextEdit
@onready var local_id: TextEdit = $VBoxContainer/TextEdit

func _ready() -> void:
	local_id.text = NetworkManager.node_id()
	$VBoxContainer/Label.text = "Local Multiplayer Peer ID: " + str(multiplayer.get_unique_id())

func _on_button_pressed() -> void:
	NetworkManager.connect_to_node(input.text)


func _rpc_test() -> void:
	this_is_a_test.rpc(randi())

@rpc("any_peer")
func this_is_a_test(random: int) -> void:
	print(multiplayer.get_unique_id(), "\n", random, "\nFrom: ", multiplayer.get_remote_sender_id())
