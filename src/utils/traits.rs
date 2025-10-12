use godot::{
    classes::Engine,
    obj::{Gd, Inherits, Singleton},
    prelude::GodotClass,
};

pub trait EngineSingleton: GodotClass {
    const SINGLETON: &'static str;

    fn singleton() -> Option<Gd<Self>>
    where
        Self: Inherits<godot::prelude::Object>,
    {
        match Engine::singleton().get_singleton(Self::SINGLETON) {
            Some(singleton) => Some(singleton.cast::<Self>()),
            None => None,
        }
    }
}
