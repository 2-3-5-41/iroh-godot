use std::path::Path;

use flatc_rust;

fn main() {
    println!("cargo::rerun-if-changed=flatbuffers/");

    flatc_rust::run(flatc_rust::Args {
        lang: "rust",
        inputs: &[Path::new("flatbuffers/godot_peer_data.fbs")],
        out_dir: Path::new("src/"),
        ..Default::default()
    })
    .expect("flatc");
}
