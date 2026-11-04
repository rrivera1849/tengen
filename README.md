# Tengen

A calm place to play and review Go on any server. Mac first; Windows and Linux later.

Tengen (天元) is the center point of the board: every server meets in one place. It's the project's codename; the public name is still to be chosen.

## Status

Early, but playable. What works today:

- A board with four textures (kaya, shin-kaya, bamboo, ink on paper), shell-and-slate or flat stones, light and dark mode, and a soft stone sound
- Full rules: captures, ko and suicide
- Single-click play, pass, undo, and stepping through any game
- Open and save SGF files. Saves default to `~/Documents/kaya`, named like `2026-11-04 21.34 OGS Rafael (3d) vs Opponent (2k).sgf`
- Load any OGS game by number or link, for review or to download its SGF
- Sign in to OGS and play live: automatch on 9×9, 13×13 or 19×19 at blitz, rapid or live pace, with clocks, pass, resign and accepting the score

Next up: local KataGo analysis, the point estimator, then Fox.

## Run it on your Mac

You need [Rust](https://rustup.rs), [Node.js](https://nodejs.org) 20 or later, and the Xcode Command Line Tools (`xcode-select --install`).

```sh
npm install
npm run tauri dev      # opens the app with live reload
npm run tauri build    # makes Tengen.app and a .dmg in src-tauri/target/release/bundle
```

`npm run dev` alone opens the interface in a browser with a sample position, which is handy for design work. Add `?live` to the address to preview a live game with clocks.

## Signing in to OGS

OGS signs apps in through the browser (OAuth2 with PKCE), so Tengen never sees your password. Tengen needs an OGS app registration once:

1. Sign in at online-go.com and open [online-go.com/oauth2/applications](https://online-go.com/oauth2/applications/).
2. Register a new application:
   - **Name:** Tengen
   - **Client type:** Public
   - **Authorization grant type:** Authorization code
   - **Redirect URIs:** `http://127.0.0.1:47613/oauth/callback` (exactly this)
3. Copy the **client ID** into `crates/tengen-ogs/src/auth.rs`:

   ```rust
   const DEFAULT_CLIENT_ID: Option<&str> = Some("paste-the-client-id-here");
   ```

   A public client has no secret, so the ID is safe to commit. To try a different registration without editing the file, build with `TENGEN_OGS_CLIENT_ID=… npm run tauri dev`.

Then choose **Play online** (⌘L) and **Sign in with OGS**. Your browser opens OGS's sign-in page and hands control back to Tengen when you approve. The sign-in is kept in the macOS Keychain and renewed automatically; it lasts 30 days between uses.

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
| ⌘L | Play online |

## How it's built

```
src/                  Interface: TypeScript, drawn as SVG in the Mac web view
src-tauri/            The app shell: a thin set of commands over the crates
crates/tengen-core/   Board rules, game records, SGF, and the server-adapter interface
crates/tengen-ogs/    Online-Go.com adapter: REST, sign-in (auth.rs) and live play (realtime.rs)
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
