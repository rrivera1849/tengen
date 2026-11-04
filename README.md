# Tengen

A calm place to play and review Go on any server. Mac first; Windows and Linux later.

Tengen (天元) is the center point of the board: every server meets in one place. It's the project's codename; the public name is still to be chosen.

## Status

Early skeleton. What works today:

- A board with four textures (kaya, shin-kaya, bamboo, ink on paper), shell-and-slate or flat stones, light and dark mode, and a soft stone sound
- Full rules: captures, ko and suicide
- Single-click play, pass, undo, and stepping through any game
- Open and save SGF files
- Load any OGS game by number or link, for review or to download its SGF

Next up: OGS sign-in and live play, local KataGo analysis, the point estimator, then Fox.

## Run it on your Mac

You need [Rust](https://rustup.rs), [Node.js](https://nodejs.org) 20 or later, and the Xcode Command Line Tools (`xcode-select --install`).

```sh
npm install
npm run tauri dev      # opens the app with live reload
npm run tauri build    # makes Tengen.app and a .dmg in src-tauri/target/release/bundle
```

`npm run dev` alone opens the interface in a browser with a sample position, which is handy for design work.

## Keys

| Key | Does |
| --- | --- |
| Click | Place a stone |
| ← → | Step back or forward through the game |
| Home, End | Jump to the start or the end |
| ⌘Z | Take back the last move |
| ⌘N | New game |
| ⌘O | Open an SGF file |
| ⌘S | Save the game as SGF |
| ⌘D | Get a game from OGS |

## How it's built

```
src/                  Interface: TypeScript, drawn as SVG in the Mac web view
src-tauri/            The app shell: a thin set of commands over the crates
crates/tengen-core/   Board rules, game records, SGF, and the server-adapter interface
crates/tengen-ogs/    Online-Go.com adapter
crates/tengen-fox/    Fox adapter (protocol research not started)
```

Every server plugs into the same `ServerAdapter` interface in `tengen-core`, so the interface never knows which server a game came from. Logins stay on your Mac, and connections go straight from your Mac to each server.

## Tests

```sh
cargo test                                   # rules, SGF, adapters
cargo test -p tengen-ogs -- --ignored        # also downloads a real game from OGS
```

## Fox

Fox has no public API, so its adapter will be built by studying the official client's traffic. That goes against Fox's terms of service, and accounts used with an unofficial client could be banned. Tengen will behave like the official client and never automate play.

## License

MIT
