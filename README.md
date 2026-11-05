# T3 Code for World of Warcraft

Chat with your [T3 Code](https://github.com/pingdotgg/t3code) agents without leaving the game.

Browse projects and threads, read replies, send messages, start threads, stop a run, and answer agent questions — from inside WoW. It works like the T3 mobile app.

Your T3 environment can be anywhere: this PC, a server, or a laptop reached over T3 Connect, Tailscale, or your LAN.

```
WoW (addon)  <-- files -->  t3wow bridge  <-- HTTPS / MCP -->  T3 Code environment
  T3Wow                    (this computer)                    (T3 Connect, Tailscale, LAN)
```

## Quick start

1. Make sure the environment runs **T3 Code 0.0.46+** (nightly for now — see [Requirements](#requirements)).
2. Download the app from the [latest release](https://github.com/DSilence/t3wow/releases/latest) and open it. It installs the app and the WoW addon, then opens **Settings**.
3. In **Settings**, paste your MCP URL and click **Sign in** (details below).
4. Start WoW and type `/t3`.

> New here? Do the steps above first. [How it works](#how-it-works) explains the bridge later — you do not need it to get started.

## Contents

- [Requirements](#requirements)
- [Install](#install)
- [Using it in game](#using-it-in-game)
- [How it works](#how-it-works)
- [Configuration](#configuration)
- [Development](#development)
- [Limitations](#limitations)
- [License](#license)

## Requirements

- **T3 Code 0.0.46 or newer** on the environment. The bridge uses its MCP endpoint for outside agents (`/mcp` with OAuth). That endpoint is not in 0.0.45.
- The **T3 WoW app** (macOS / Windows) or the `t3wow` binary (Linux) on the PC that runs WoW.
- **WoW Retail or Classic.**

## Install

### 1. Download and open the app

Get the file for your system from the [latest release](https://github.com/DSilence/t3wow/releases/latest):

| System | File | First-open warning |
| --- | --- | --- |
| macOS | `T3-WoW-macOS.zip` | Open the zip, double-click **T3 WoW**. If macOS blocks it (the app is not notarized): **System Settings > Privacy & Security** > scroll to the T3 WoW message > **Open Anyway**. |
| Windows | `T3-WoW-Windows.exe` | If SmartScreen warns: **More info > Run anyway**. |
| Linux | `t3wow` binary | No tray app — use the [command line](#command-line-and-linux) instead. |

Opening the installer does three things:

1. Installs the **T3 WoW** app — `~/Applications` on macOS, `%LOCALAPPDATA%\t3wow` on Windows (with a Start Menu entry).
2. Installs the addon into every WoW folder it finds.
3. Starts the app and opens **Settings**.

You can delete the download afterwards. Opening a newer download later updates the app in place.

### 2. Connect to your T3 environment

In **Settings**:

1. **T3 Code environment** — paste the MCP URL and click **Sign in**.
   - Find the URL in T3 Code: **Settings > Connections** > open the environment menu > **Copy MCP URL**.
   - Use the T3 Connect address if you want to reach it from anywhere.
   - Your browser opens an approval page. Enter a pairing code from the host (**Settings > Connections**, or `t3 auth pairing create`) and pick an access level.
   - Pick **Supervised** or higher to send messages. **Read only** can only watch threads.
   - Threads started in game cannot use more permission than you approve here. Sign-in lasts 30 days.
2. **World of Warcraft** — check the folders (`_retail_`, `_classic_`, …) where the addon should be installed. Uncheck to remove it. Use **Add World of Warcraft folder…** if your game is in an unusual place.
   - Change this only while the game is closed, then restart the game. WoW only finds new addon folders at launch.
3. **Live updates** — leave at the default (300). This is how many times per session the addon can refresh without a `/reload` (see [How it works](#how-it-works)). On Windows each update is a copied file, so very high numbers can slow updates down.

### 3. Run it

The app lives in the menu bar (macOS) or notification area (Windows):

- **Ring** = waiting for WoW · **Disc** = syncing · **Ring with a dot** = something needs you.
- The menu shows bridge status and has **Settings…**, **Check for updates**, **Open log**, and **Quit**.
- Opening the app again while it runs re-opens Settings.

Nothing starts on its own. Open the app when you want the bridge. While it runs:

- It waits for WoW, syncs while you play, and delivers queued messages when you log out.
- Settings changes apply right away, even mid-game.
- Only one bridge runs at a time. The log is at `~/.t3wow/bridge.log`.

> **macOS, WoW on an external drive?** If Settings or the log says “cannot write”, allow **T3 WoW** under **System Settings > Privacy & Security > Files and Folders** (or **Full Disk Access**).

### Updates

The app updates itself. It checks shortly after start and every 6 hours, verifies the download against the release `SHA256SUMS`, and installs it once WoW is closed (addon included), then restarts. Use **Check for updates** in the menu or Settings to check now. Turn off **Install updates automatically** in Settings to update only manually.

### Command line and Linux

Every release also ships a plain `t3wow` binary (`t3wow-<target>`) that does everything from a terminal. On Linux this is the only option:

```sh
curl -fLo ~/.local/bin/t3wow https://github.com/DSilence/t3wow/releases/latest/download/t3wow-x86_64-unknown-linux-gnu
chmod +x ~/.local/bin/t3wow
t3wow setup https://<environment-address>/mcp   # sign in and install the addon
t3wow run --wait-for-wow                        # run the bridge in this terminal
```

Common commands:

| Command | What it does |
| --- | --- |
| `t3wow setup <mcp-url>` | Sign in + install the addon (+ the tray app on macOS/Windows). |
| `t3wow login <mcp-url>` | Sign in only. |
| `t3wow install` | Install the addon only. Finds WoW itself, or use `--wow "/path/to/World of Warcraft"`. |
| `t3wow status` | Show the current setup. |
| `t3wow update` | Install the latest release (close WoW first; `--check` only looks). |
| `t3wow run` | Sync right away. Add `--wait-for-wow` to wait for the game. |

Useful `install` flags: `--flavor classic_beta`, `--slots 300`, `--link` (symlink this checkout’s addon for development), `--copy-slots`. Set `WOW_DIR` to point at your WoW folder when it cannot be found. Checksums for every release are in `SHA256SUMS`.

To build from source instead, install [Rust](https://rustup.rs), then run `cargo build --release --manifest-path bridge/Cargo.toml`. The binary lands at `bridge/target/release/t3wow`.

> Upgrading from an old version that installed a login service? Installing the app removes it. The bridge will not sync while that service is still present.

In game, `/t3` opens the window. You can also bind a key under **Key Bindings > AddOns**.

## Using it in game

Type `/t3` to open the window.

**Layout**

- **Left — views, projects, and threads.** **Inbox** collects finished threads and questions that need you; **Working** shows busy agents; **Settled** shows threads marked done across projects. **Projects** lets you browse one project at a time. Click **+ New** to start a thread — the first line of your message becomes its title. The **project** button next to the model chooses where a new thread runs.
- **Search.** Type in the sidebar to filter threads by title, project, model, or branch. Press Enter to search T3 for matching titles, branches, PRs, projects, and messages too (requires an environment with thread search).
- **Right — the conversation.** Markdown renders in game: headings, lists, quotes, code blocks, tables, **bold**, *italics*, `code`, and links. Click **Copy** next to a message to copy its text (Ctrl+C), or click a link to copy the URL. Click **Stop** to interrupt a running turn. Click **Answer** to reply to an agent question — type an option number or your own text.
- **Composer (bottom).** Press Ctrl+Enter or **Send** to send. Use **Auto / Queue / Steer** to choose what happens if the agent is already working. Use the **model** and **reasoning** buttons to change what the thread uses from your next message on.
- **Header.** **Compact** summarizes the conversation to free model context (like `/compact`; it waits for a running turn). **Settle** marks the thread done and moves it to Settled; **Unsettle** brings it back. Settling needs an environment that supports it.
- **Footer.** Bridge status, how fresh the data is, live updates left, and unsent commands.

**Sync** fetches the latest data without reloading when it can. It only reloads the UI when it must: to send queued commands, to fully load a thread that is only previewed, or when live updates run out. Shift-click **Sync** (or `/t3 reload`) to always reload.

The bridge fully syncs the thread you have open, threads you opened recently, running threads, and the most recent few. Up to 16 more threads get a short preview so opening one shows something instantly — press **Sync** to load it in full.

**Minimap button:** click to open/close, right-click to open the latest finished thread, drag to move it. A green dot means something is running; the number counts finished threads you have not opened yet.

**Notifications:** when a thread finishes or an agent needs you, you get a chat line, a sound, a flashing taskbar icon, and a toast at the top of the screen. Click the toast to open the thread. Finished-but-unopened threads are marked **new**. This works with the window closed: while anything runs, the addon refreshes every 10 seconds in the background, and finishes that happen during a reload are still noticed.

**Slash commands**

| Command | What it does |
| --- | --- |
| `/t3` | Open or close the window. |
| `/t3 sync` | Fetch the latest data. |
| `/t3 reload` | Reload the UI (always). |
| `/t3 send <text>` | Send a message to the open thread. |
| `/t3 compact` | Compact the open thread. |
| `/t3 settle` / `/t3 unsettle` | Mark the open thread done / bring it back. |
| `/t3 inbox` / `/t3 working` / `/t3 settled` / `/t3 projects` | Switch the sidebar view. |
| `/t3 search <text>` | Open search with the given text. |
| `/t3 status` | Show bridge and sync status. |
| `/t3 autoreload on\|off` | Queue several messages and send them together. |
| `/t3 interval <sec>` | Set the background refresh interval. |
| `/t3 font <size>` | Set the font size. |
| `/t3 notify on\|off` | Turn notifications on or off. |
| `/t3 sound on\|off` | Turn the notification sound on or off. |
| `/t3 minimap on\|off` | Show or hide the minimap button. |
| `/t3 fastsend on\|off` | Turn screenshot sending on or off. |
| `/t3 clear` | Clear the window. |
| `/t3 reset` | Reset addon settings. |

Sending works in combat. If a message must fall back to a UI reload, it never reloads in combat — it stays queued until you press **Sync**.

## How it works

WoW addons cannot use the network and cannot read files while the game runs. So a small bridge program (~2 MB, written in Rust) runs next to the game, does the networking, and talks to the addon through files WoW already loads:

- **T3 → game:** the bridge rewrites `T3Wow_Inbox/Data.lua` and a set of load-on-demand `T3Wow_LiveNNN` addons. Each live addon can load once per UI session, so the open window loads the next one every few seconds and stays fresh **without** `/reload`. That is what “live updates” count in Settings.
- **Game → T3:** normally commands sit in SavedVariables (`T3WowDB.outbox`), but WoW only writes those on `/reload` or logout. So the addon **sends by screenshot** instead: it draws the data as a grid of colored cells in the top-left corner for a frame or two, takes a screenshot, and the bridge reads and deletes it. If that fails, or the bridge does not answer in 20 seconds, it falls back to a UI reload.

Each command carries an idempotency key, so it is delivered exactly once even if the bridge restarts. The addon removes it once a snapshot confirms delivery, and failures appear in the thread and in chat.

This uses only normal addon features: files at UI load, SavedVariables, and screenshots. It does not read game memory or the screen, and it does not send input to the game. Prefer reloads? Run `/t3 fastsend off` to turn screenshot sending off.

## Configuration

Settings live in `~/.t3wow/config.json`, written by Settings, `login`, and `install`. The bridge picks up changes while it runs. The first and last keys are also in Settings; the rest are for hand-tuning:

| Key | Default | Meaning |
| --- | --- | --- |
| `liveSlots` | 300 | Live updates per UI session (Settings, or `install --slots`). |
| `activePollMs` | 2000 | Poll interval while something runs or is sending. |
| `idlePollMs` | 8000 | Poll interval otherwise. |
| `maxProjects` | 12 | How many projects to include. |
| `threadsPerProject` | 20 | Threads listed per project. |
| `recentThreadsWithMessages` | 4 | Recent threads whose messages are always synced. |
| `messagesPerThread` | 30 | Messages kept per thread. |
| `maxCharsPerMessage` | 6000 | Longer messages are shortened (open T3 Code for the full text). |
| `oauthPort` | 47823 | Local port for the sign-in redirect. |
| `autoUpdate` | true | Let the tray app install new releases on its own. |

Your sign-in token is in `~/.t3wow/auth.json` (mode 600). Set `T3WOW_HOME` to use a different folder.

## Development

```sh
# Bridge (bridge/, Rust)
cargo test --manifest-path bridge/Cargo.toml     # unit tests + end to end
cargo run --manifest-path bridge/Cargo.toml -- status

# Addon (addon/, Lua; tests in TypeScript on fengari)
pnpm install
pnpm test          # addon tests (the addon runs in a Lua VM with stubbed WoW APIs)
pnpm typecheck
pnpm lint:lua      # Lua 5.1 syntax + accidental globals
```

What the tests cover:

- `bridge/tests/e2e.rs` — the full loop against a fake T3 MCP server: SavedVariables → bridge → MCP → snapshot files → the real addon in Lua 5.1.
- `bridge/tests/wait_for_wow.rs` — tray wait-for-WoW mode (idle while the game is closed, final delivery at logout).
- `bridge/tests/settings_change.rs` — settings changing while the game runs.
- `bridge/tests/settings_window.rs` — the commands the Settings page sends. The page is `bridge/assets/settings.html`, shown in a web view (WKWebView / WebView2). Try it with `cargo run --manifest-path bridge/Cargo.toml -- tray --settings` (set `T3WOW_HOME` and `WOW_DIR` for a scratch setup).

To type-check the Linux and Windows paths from macOS without a C cross-compiler, skip TLS (its crypto library needs one):

```sh
cargo clippy --no-default-features --target x86_64-pc-windows-gnu
```

### Releasing

CI (`.github/workflows/ci.yml`) runs the bridge tests on macOS, Linux, and Windows plus the addon tests on every push and pull request.

To publish, run the **Release** workflow from the Actions tab with a version, or:

```sh
gh workflow run release.yml -f version=0.2.0
```

It bumps the version in `bridge/Cargo.toml`, `Cargo.lock`, and `package.json`, tags `v0.2.0`, builds for macOS (Apple silicon + Intel), Windows, and Linux (x86_64 + ARM), and publishes the binaries, `T3-WoW-macOS.zip`, `T3-WoW-Windows.exe`, and `SHA256SUMS`. Pushing a `vX.Y.Z` tag that matches `bridge/Cargo.toml` releases that commit instead. Installed apps pick the release up within 6 hours.

Keep asset names (`t3wow-<target>[.exe]`) in sync with what the updater looks for in `bridge/src/update.rs`.

## Limitations

- **Screenshot sending is visible.** It briefly shows a colored grid in the top-left corner and causes a short hitch while the image saves. The format switches to JPEG for that moment, then restores. About 2.3 KB fits in one screenshot; longer messages use several. With it off (or when it fails), sending reloads the UI.
- **Tool permission prompts cannot be approved over MCP.** Approve the bridge with **Auto** or **Full access** if game-started threads should run unattended.
- **WoW fonts are limited.** No emoji and some scripts are missing. Bold and italics appear as colours (one line cannot switch fonts mid-line), and rendered text is not selectable — use **Copy**.
- **Live updates depend on symlinks on macOS/Linux.** Each slot’s `Data.lua` links to one shared file, so a snapshot is written once. If a client does not follow links (updates never arrive), reinstall with `--copy-slots`. Windows always copies.
- **Live updates need per-client verification.** If a client caches addon files, updates still arrive via **Sync** or `/reload`.

## License

[MIT](LICENSE). Not affiliated with Blizzard Entertainment or T3 Code.
