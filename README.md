# iroh-godot

**EXPERIMENTAL**: This project is under experimental development, use at your own risk.

`iroh-godot` is a Godot extension that adds a new `MultiplayerPeer` implementation for the [iroh](https://iroh.computer) networking library to take advantage of Godot's high-level multiplayer api.

## Build

Building from source is simple, if we assume you already know how to use git, and the terminal.

- Clone this repo
- Open the newly cloned repo in your terminal
- Run `./build.sh`
- Copy `addons/` from the root of this project into your own godot project
- Run your project, and start building

## Mentions

- [godot-iroh](https://github.com/tipragot/godot-iroh)
  - For proving the implementation of `iroh` into godot as a `MultiplayerPeerExtension`.
