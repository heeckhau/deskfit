# Deskfit

Control a Sportstech DeskFit200 under-desk treadmill from macOS with an
Elgato Stream Deck +: rotate a dial to change speed, press it to start or
pause, and see state and speed on the touch strip. Everything runs locally
over Bluetooth Low Energy.

> [!WARNING]
> This is an unofficial hobby project, not affiliated with Sportstech,
> InnovaMaxx or Elgato. It drives a motor you stand on, using a reverse
> engineered protocol. Use it at your own risk; there is no warranty.
> Keep the treadmill's hardware remote within reach: software pause is not
> an emergency stop (pressing start twice on the remote reportedly is).
> The treadmill's Bluetooth interface is unauthenticated, so anyone in
> range can control it.

## Features

- **Stream Deck + dial:** each tick changes speed by one position (1-8 km/h,
  like the remote), only while the belt is running. Pressing the dial
  starts or pauses. Rapid rotation is coalesced and a pause always takes
  priority.
- **Touch strip:** state (running, slowing down, paused, connecting,
  unknown), speed, pending target, and a speed bar. A lost connection shows
  as unknown, never as a stopped belt.
- **Setting:** maximum speed position (default 8).
- **Developer CLI:** scan, status, watch and motor commands for testing.

The plugin never starts or resumes the belt on its own, including after
reconnecting.

## Status

Verified on a DeskFit200: status polling, pause, resume from pause, and
setting the speed while running. See [docs/protocol.md](docs/protocol.md)
for the protocol and what was verified.

Not yet done:

- Hardware tests of stop, start from stopped, set speed while paused, and
  the auto-stop after a long pause.
- Checks of reconnecting after Bluetooth loss, Mac sleep/wake, and the
  treadmill being switched off.
- An installable `.streamDeckPlugin` package; for now the plugin is linked
  from a source build.

## Requirements

- macOS with Bluetooth, Stream Deck 7.1 or later, and a Stream Deck +
  (keys work too, without the dial).
- Rust, Node.js 24 and npm to build.
- Optionally an Apple Development signing identity, so that macOS keeps the
  helper's Bluetooth permission across rebuilds
  ([why](docs/macos-bluetooth.md)).

## Setup

Build the CLI and pick your treadmill (power it on first; your terminal
needs Bluetooth permission):

    cargo build --release
    target/release/deskfit scan
    target/release/deskfit setup          # or: setup --id <identifier from scan>
    target/release/deskfit status

Build and install the plugin; its npm scripts live in `plugin/`:

    cd plugin
    npm install
    npm run build:helper    # build, copy and sign the helper into the plugin
    npm run build           # bundle the TypeScript plugin
    npx streamdeck link org.eeckhaut.deskfit.sdPlugin

Restart Stream Deck, drag **Deskfit > Treadmill** onto a dial, and allow
Bluetooth access when macOS asks.

After changing code, from the repository root:

    npm --prefix plugin run build:helper && npm --prefix plugin run build
    (cd plugin && npx streamdeck restart org.eeckhaut.deskfit)

## CLI

    deskfit status | watch            # read state; add --json for JSON lines
    deskfit pause | start | stop      # then prints status for a few seconds
    deskfit speed 30                  # tenths of km/h, 10-80

The treadmill accepts one connection at a time, so the CLI cannot connect
while the plugin is running.

## Architecture

    Stream Deck + <-> TypeScript plugin <-> Rust helper <-> BLE treadmill

The plugin starts `deskfit serve` as a child process and exchanges JSON
lines with it over stdin/stdout. The helper owns the Bluetooth connection,
polls status twice a second, and exits when the plugin goes away. It
launches itself as its own "responsible process" so that macOS asks for
Bluetooth permission for the helper rather than for Stream Deck
([details](docs/macos-bluetooth.md)).

## Acknowledgements

The command set comes from [machinekoder/deskfit](https://github.com/machinekoder/deskfit)
and [leoluk/dft200-go](https://github.com/leoluk/dft200-go).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
