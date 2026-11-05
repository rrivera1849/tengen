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
- Sign in to Fox and play live: automatch on 9×9, 13×13 or 19×19, clocks, and Fox's own counting at the end
- A score estimate on one key (E), from KataGo running on your Mac: the score, who owns what, and dead stones
- An AI review (R) of any finished game: a score graph, up to five of your mistakes and three of your opponent's, each with KataGo's better line, a plain-language lesson from Claude, and a maxim to carry into your next game. Saved SGFs keep the lessons as comments

KGS, Pandanet and Fox are new and haven't been through full games on the real servers yet; expect a fix or two after the first games.

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
3. Copy `tengen.local.example.toml` to `tengen.local.toml` (git ignores it) and paste the **client ID** in:

   ```toml
   ogs_client_id = "paste-the-client-id-here"
   ```

   Rebuild after changing it. You can also set `TENGEN_OGS_CLIENT_ID=…` when building instead; the environment variable wins.

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

The review circles your mistakes on the board in orange and your opponent's in blue; click one to read its lesson. **Try again** sets up each position before your mistakes so you can look for the better move yourself, with three tries and a hint, before KataGo's line is shown. The graph under the board shows who led, and dragging along it moves through the game.

Analyses and reviews are cached in Tengen's Application Support folder, so reopening a review is instant. Your maxims collect there too, and a review notices when the same kind of lesson keeps coming back.

## Keys

| Key | Does |
| --- | --- |
| Click | Place a stone; while scoring on KGS or Pandanet, mark a group dead or alive |
| P | Pass |
| E | Show or hide the score estimate |
| R | Review the game |
| Z | Zen mode: only the board and the clocks |
| Esc | Hide the estimate or a better line, leave a try or a lesson, close the review, leave zen mode |
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
crates/tengen-fox/    Fox adapter: Fox's binary protocol over TCP (see PROTOCOL.md)
```

Every server plugs into the interfaces in `tengen-core` (`ServerAdapter` for records, `LiveClient` and `LiveEvent` for live play), so the interface never knows which server a game came from. Each adapter keeps its protocol in a `protocol.rs` with no networking, tested line by line. Logins stay on your Mac, and connections go straight from your Mac to each server.

## Tests

```sh
cargo test                                   # rules, SGF, adapters
cargo test -p tengen-ogs -- --ignored        # also downloads a real game from OGS
TENGEN_KATAGO=… TENGEN_KATAGO_MODEL=… cargo test -p tengen-ai --test real_katago   # against a real KataGo
```

## Fox

Choose **Play online** (⌘L), pick **Fox**, and sign in with your Fox account name and password. Fox's apps let you sign in by phone, WeChat or Google without ever setting a password; if that's your case, make an account with a password at Fox's [overseas sign-up page](https://account.foxwq.com/reg/foreign.html), which only needs an email.

Fox ends games by counting, not by marking dead stones. When both players pass, Tengen asks Fox to count; once your opponent agrees, Fox's AI judges the board, Tengen shows the stones it considers dead, and **Accept score** confirms. If your opponent asks to count, **Accept score** agrees.

Fox has no public API. Tengen speaks its protocol as written down in [`crates/tengen-fox/PROTOCOL.md`](crates/tengen-fox/PROTOCOL.md), learned from [foxwq-web](https://github.com/foxwq-web/foxwq-web.github.io), an open browser client for Fox. Fox doesn't allow unofficial clients, so an account used with Tengen could be restricted. Tengen behaves like a person using the official client and never plays on its own.

## License

MIT
