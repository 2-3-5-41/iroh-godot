use godot::prelude::*;

mod godot_peer_data_generated;
mod multiplayer_peer;
mod protocol;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {}
