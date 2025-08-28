use godot::prelude::*;

mod api;
mod extension;
mod proto;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {}
