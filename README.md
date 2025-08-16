# iroh-godot

**EXPERIMENTAL**: This project is under experimental development, use at your own risk.

`iroh-godot` is a GDExtension that adds a new `MultiplayerPeer` implementation for the [iroh](https://iroh.computer) networking library to take advantage of Godot's high-level multiplayer api, allowing for seamless* integration of peer-2-peer connectivity in your multiplayer project.

> *Seamless in a sense that you can just swap out your current `ENETMultiplayerPeer`, or other implementation, with this one and continue using Godot's build in multiplayer APIs/Nodes with minimal changes to your actual code. You _may_ have to change how you thinkg about your use of RPC channels, since you're no longer in the world of TCP/UDP sockets.

## Build

Building from source is simple, if we assume you already know how to use git, and the terminal.

- Clone this repo
- Open the newly cloned repo in your terminal
- Run `./build.sh`
- Copy `addons/` from the root of this project into your own godot project
- Open your project, and start building

## Mentions

- [godot-iroh](https://github.com/tipragot/godot-iroh)
  - For proving the implementation of `iroh` into godot as a `MultiplayerPeerExtension`.
- [godot-rust/gdext](https://github.com/godot-rust/gdext)
  - Intuitive Rust GDExtension bindings.
- [flatbuffers](https://flatbuffers.dev)
  - For making data serialization/deserialization fast and simple.
- [iroh](https://iroh.computer)
  - For making peer-2-peer simple.

## Limitations*

- Currently only supports reliable QUIC bi-directional streams between peers.
  - *Even though QUIC streams are considerably fast at transmitting data, some would (and, kind of should) argue a raw UDP socket _is_ faster for just sending packets out without regard to when it was sent.

## Plans

- Since QUIC streams already handle multiplexing (channels in godot's TCP/UDP multiplexing imlementation), there's the plan to change how channels on RPCs are used; specifically, to dictate which iroh protocol (i.e [`iroh-gossip`](https://www.iroh.computer/proto/iroh-gossip)) to transmit a packet over.
- Using [`iroh-gossip`](https://www.iroh.computer/proto/iroh-gossip) protocol under the hood to implement host migration.
