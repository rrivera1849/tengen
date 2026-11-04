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
- Sign in to KGS or Pandanet (IGS) with your name and password and play live the same way: 19×19 automatch, clocks (including Canadian overtime), and clicking dead stones while scoring
- A score estimate on one key (E), from KataGo running on your Mac: the score, who owns what, and dead stones
- An AI review (R) of any finished game: a score graph, up to five of your mistakes and three of your opponent's, each with KataGo's better line, a plain-language lesson from Claude, and a maxim to carry into your next game. Saved SGFs keep the lessons as comments

KGS and Pandanet are new and haven't been tried against the real servers yet; expect a fix or two after the first games. Next up: Fox.

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

## Playing on KGS and Pandanet

Both sign in with your account's name and password, which Tengen keeps in the macOS Keychain and sends only to that server. Choose **Play online** (⌘L), pick **KGS** or **Pandanet**, and sign in once; Tengen signs in again by itself on launch.

- **KGS:** Tengen uses KGS's official [JSON API](https://www.gokgs.com/json/protocol.html). No account yet? At [gokgs.com](https://www.gokgs.com), enter as a guest, then choose **Register**; KGS emails you a password. KGS automatch is 19×19 only and for kyu players.
- **Pandanet:** Tengen speaks the IGS text protocol at `igs.joyjoy.net:6969`, the same one desktop IGS clients use. Register at [pandanet-igs.com](https://www.pandanet.co.jp/English/register/index.htm). Pandanet's automatch ("seek") is 19×19 only; Blitz, Rapid and Live pick the shortest, middle and longest of the time settings it offers.

When both players pass, click stones to mark them dead (click again to bring them back), then **Accept score**. On Pandanet, bringing stones back resets every stone marked so far, because that's how IGS works.

## Score estimate and AI review

Both run on KataGo, the open-source Go engine, on your own Mac. KataGo decides what is true; Claude only explains it. Each lesson is written from a fact sheet Tengen has computed (the move, the better line, points lost, which groups lived or died), and Tengen checks every coordinate and number in it before you see it. A lesson that fails twice falls back to KataGo's facts alone.

**KataGo.** KataGo publishes no Mac builds, so the **KataGo runtime** workflow (Actions tab, *Run workflow*) builds it with its Metal backend, adds a network, and publishes the `katago-runtime` release. Tengen offers to download it the first time you press E or R, and checks each file's SHA-256. Run the workflow once before trying it. While this repository is private, those downloads need a GitHub sign-in, so either make the repository public or point Tengen at a KataGo you already have:

```sh
brew install katago   # Tengen finds Homebrew's katago by itself; it still needs a network file:
TENGEN_KATAGO=/opt/homebrew/bin/katago TENGEN_KATAGO_MODEL=~/Downloads/kata1-b18c384nbt.bin.gz npm run tauri dev
```

**Claude.** Written lessons need a Claude API key from [console.anthropic.com](https://console.anthropic.com). Add it from the review panel; it's stored in the macOS Keychain. Without a key the review still shows the score graph and KataGo's better lines. Only positions and KataGo's facts are sent, never player names.

Analyses and reviews are cached in Tengen's Application Support folder, so reopening a review is instant. Your maxims collect there too, and a review notices when the same kind of lesson keeps coming back.

## Keys

| Key | Does |
| --- | --- |
| Click | Place a stone; while scoring on KGS or Pandanet, mark a group dead or alive |
| P | Pass |
| E | Show or hide the score estimate |
| R | Review the game |
| Esc | Hide the estimate, a better line, or the review |
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
crates/tengen-kgs/    KGS adapter: the JSON API over HTTP long polling
crates/tengen-igs/    Pandanet adapter: the IGS text protocol over TCP
crates/tengen-ai/     KataGo engine bridge, score estimate, lesson finder, Claude client and maxims
crates/tengen-fox/    Fox adapter (protocol research not started)
```

Every server plugs into the interfaces in `tengen-core` (`ServerAdapter` for records, `LiveClient` and `LiveEvent` for live play), so the interface never knows which server a game came from. Each adapter keeps its protocol in a `protocol.rs` with no networking, tested line by line. Logins stay on your Mac, and connections go straight from your Mac to each server.

## Tests

```sh
cargo test                                   # rules, SGF, adapters
cargo test -p tengen-ogs -- --ignored        # also downloads a real game from OGS
TENGEN_KATAGO=… TENGEN_KATAGO_MODEL=… cargo test -p tengen-ai --test real_katago   # against a real KataGo
```

## Fox

Fox has no public API, so its adapter will be built by studying the official client's traffic. That goes against Fox's terms of service, and accounts used with an unofficial client could be banned. Tengen will behave like the official client and never automate play.

## License

MIT
