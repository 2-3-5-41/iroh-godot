use godot::{classes::Engine, prelude::*};

// Internal Modules
mod network;
mod utils;

struct IrohGodot;

#[gdextension]
unsafe impl ExtensionLibrary for IrohGodot {
    fn on_level_init(level: InitLevel) {
        match level {
            InitLevel::Scene => {
                // Enable multi-thread logging.
                tracing_subscriber::fmt().init();
            }
            _ => (),
        }
    }
}
