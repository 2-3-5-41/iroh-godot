use godot::prelude::*;

mod api;
mod extension;
mod proto;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {
    fn on_level_init(level: InitLevel) {
        match level {
            InitLevel::Scene => {
                tracing_subscriber::fmt().init();
            }
            _ => (),
        }
    }
}
