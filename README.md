# iroh-godot

**EXPERIMENTAL**: This project is under experimental development, use at your own risk.

`iroh-godot` is a Godot extension that adds a new `MultiplayerPeer` implementation for the [iroh](https://iroh.computer) networking library to take advantage of Godot's high-level multiplayer api.

## Build

Building from source is simple, if we assume you already know how to use git, and the terminal.

- Clone this repo
- Open the newly cloned repo in your terminal
- Run `./build.sh`
  - The build script will build both debug, and release binaries
  - The build script will then create symlinks of the built `debug` and `release` directories to the repo's `addons/iroh-godot` directory.
- Create a symlink, or copy the `addons/iroh-godot` directory into your Godot project.
