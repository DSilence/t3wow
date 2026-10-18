# T3 Code for World of Warcraft

Talk to your [T3 Code](https://github.com/pingdotgg/t3code) agents from inside
the game. Browse projects and threads, read the conversation, send messages,
start new threads, stop a run and answer agent questions. The T3 environment
can run on any machine: your desktop, a server, or a laptop reachable over
T3 Connect, Tailscale or your LAN. This works like the T3 mobile app.

```
 WoW (addon)  <-- files -->  t3wow bridge  <-- HTTPS / MCP -->  T3 Code environment
   T3Wow                    (this computer)                    (T3 Connect, Tailscale, LAN)
```

## Why a bridge?

WoW addons are sandboxed: they cannot open network connections and cannot
read files while the game runs. The bridge is a small program (one ~2 MB
binary, written in Rust) running next to the game. It does the networking and
exchanges data with the addon through files the game loads:

| Direction        | How                                                                                                                                                                                                                                                     |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T3 to game       | The bridge rewrites `T3Wow_Inbox/Data.lua`, which loads with the UI, and the load-on-demand `T3Wow_LiveNNN` addons. Each live addon can be loaded once per UI session, so while the window is open the addon loads the next one every few seconds and gets fresh data **without** `/reload`. |
| Game to T3       | Commands go into the addon's SavedVariables (`T3WowDB.outbox`). WoW writes SavedVariables only on `/reload` or logout, so the addon **sends by screenshot** instead: it draws the outbox as a grid of colored cells in the top-left corner for a frame or two, takes a screenshot, and the bridge decodes and deletes it. If that fails or the bridge does not answer within 20 seconds, it falls back to reloading the UI, which writes SavedVariables. |

Each command carries an idempotency key, so it is delivered only once even if
the bridge restarts. The addon drops it from the outbox when a snapshot
acknowledges it, and failures show in the thread and in chat.

This uses only what addons may normally do: files loaded at UI load,
SavedVariables and the screenshot function. Nothing reads game memory, the
screen, or sends input to the game. `/t3 fastsend off` turns the screenshot
channel off if you prefer reloads.

## Requirements

- **The T3 environment must run T3 Code 0.0.46 or newer** (currently
  nightly). The bridge uses the environment's MCP endpoint for outside
  agents (`/mcp` with OAuth sign-in), which is not in 0.0.45.
- The `t3wow` binary on the computer that runs WoW. Until there are release
  builds, build it with [Rust](https://rustup.rs):
  `cargo build --release --manifest-path bridge/Cargo.toml`, which produces
  `bridge/target/release/t3wow`.
- WoW Retail or Classic (TOC lists 1.60.1 / 16001 and current retail and
  classic versions).

## Setup

Get the MCP URL from T3 Code: **Settings > Connections**, open the
environment's menu, **Copy MCP URL**. Use the T3 Connect address to reach it
from anywhere. Then:

```sh
t3wow setup https://<environment-address>/mcp
```

That does three things, which you can also run one at a time:

1. `t3wow login <mcp-url>` signs in. The approval page opens in your browser.
   Enter a pairing code from the host (**Settings > Connections** or
   `t3 auth pairing create`) and choose an access level. Pick **Supervised**
   or higher to send messages; **Read only** lets you watch threads only.
   Threads started from the game cannot use more permissions than you approve
   here. The sign-in lasts 30 days.
2. `t3wow install` installs the addon (it is built into the binary). It finds
   WoW automatically, or pass `--wow "/path/to/World of Warcraft"`. Options:
   `--flavor classic_beta`, `--slots 300`, `--link` (symlink this checkout's
   addon, for development), `--copy-slots` (see below).
3. `t3wow app install` installs the **T3 WoW** tray app: `~/Applications/T3
   WoW.app` on macOS (a menu bar app, no Dock icon), a Start Menu shortcut on
   Windows. Nothing starts on its own: open the app when you want the bridge.
   While it is open it waits for World of Warcraft, syncs while the game runs,
   delivers what you queued at logout, and goes back to waiting when the game
   closes. Its menu shows what it is doing and has **Open log** and **Quit**.
   The icon is a ring while waiting, a disc while syncing, and a ring with a
   dot when something needs you. The log is `~/.t3wow/bridge.log`.

Restart WoW once after installing (new addon folders are only picked up at
launch). Without the tray (and on Linux): `t3wow run --wait-for-wow` in a
terminal, or plain `t3wow run` to sync right away. Only one bridge runs at a
time; a second one refuses to start.

Earlier versions installed a login service instead. `t3wow app install` (and
`setup`) removes it; the tray app will not sync while it is still there.

On macOS, if WoW lives on an external drive, the app may need access to it:
allow **T3 WoW** under **System Settings > Privacy & Security > Files and
Folders** (or **Full Disk Access**) if the log shows "cannot write".

In game: `/t3` opens the window. You can also bind a key under
Key Bindings > AddOns.

## Using it

- **Left:** projects, then that project's threads (status dot, age, model).
  Use **+ New** (in a project or on the project list) to start a thread; the
  **project** button next to the model picks where it runs. The first line of
  your message becomes its title.
- **Header:** **Compact** summarizes the conversation to free the model's
  context, like `/compact` in T3 Code (it waits for a running turn).
- **Right:** the conversation, with Markdown rendered: headings, lists, quotes,
  code blocks and tables (in a bundled monospace font), bold, italics, inline
  code and links. **Copy** next to a message opens its text for Ctrl+C;
  clicking a link does the same for the URL. **Stop** interrupts a running
  turn. **Answer** replies to an agent question; type an option number or your
  own text.
- **Composer:** Ctrl+Enter or **Send**. The **Auto/Queue/Steer** button
  decides what happens when the agent is already working. The **model** and
  **reasoning** buttons pick what the thread runs from your next message on
  (or what a new thread starts with), from the models enabled in T3 Code.
- **Footer:** bridge status, data age, live updates left, and unsent
  commands.
- **Sync** loads the latest data without a reload when it can. It reloads the
  UI only when it has to: to send queued commands, to load the whole open
  thread when only a preview is synced, or when the live updates are used up.
  Shift-click (or `/t3 reload`) always reloads.

The bridge syncs full messages for the thread you have open, threads you
opened recently, running threads, and the most recent few. Up to 16 more
threads (the open project's first) get a preview of their last messages, so
opening them shows something right away; press Sync to load one in full.

**Minimap button:** click to open or close the window, right-click to open
the latest finished thread, drag to move it. A green dot means something is
running; the number counts finished threads you have not opened yet.

**Notifications:** when a thread finishes (done, failed or stopped) or an
agent needs your input, you get a chat line, a sound, a flashing taskbar icon
and a toast under the top of the screen; click the toast to open the thread.
While something runs, the addon keeps loading live updates every 10 seconds
even with the window closed, so this works in the background. Threads that
finished during a reload are noticed too. Finished threads you have not opened
are marked **new** in the list.

Slash commands: `/t3 sync`, `/t3 reload`, `/t3 send <text>`, `/t3 compact`, `/t3 status`,
`/t3 autoreload on|off` (queue several messages and send them together),
`/t3 interval <sec>`, `/t3 font <size>`, `/t3 notify on|off`,
`/t3 sound on|off`, `/t3 minimap on|off`, `/t3 fastsend on|off`, `/t3 clear`,
`/t3 reset`.

Sending by screenshot works in combat too. When it has to fall back to a
reload, that never happens in combat: the message stays queued until you press
Sync.

## Configuration

`~/.t3wow/config.json` (created by `login`/`install`; the bridge rereads it
each time the game starts):

| Key                         | Default | Meaning                                              |
| --------------------------- | ------- | ---------------------------------------------------- |
| `liveSlots`                 | 300     | Live updates per UI session (50 on Windows; re-run `install`) |
| `activePollMs`              | 2000    | Poll interval while something runs or is being sent   |
| `idlePollMs`                | 8000    | Poll interval otherwise                              |
| `maxProjects`               | 12      | Projects included                                    |
| `threadsPerProject`         | 20      | Threads listed per project                           |
| `recentThreadsWithMessages` | 4       | Recent threads whose messages are always synced      |
| `messagesPerThread`         | 30      | Messages kept per thread                             |
| `maxCharsPerMessage`        | 6000    | Longer messages are shortened (open T3 for the rest) |
| `oauthPort`                 | 47823   | Loopback port for the sign-in redirect               |

The sign-in token is stored in `~/.t3wow/auth.json` (mode 600). Set
`T3WOW_HOME` to use another directory.

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

`bridge/tests/e2e.rs` runs the whole loop against a fake T3 MCP server:
SavedVariables, then bridge, then MCP, then snapshot files, then the real
addon in Lua 5.1. `bridge/tests/wait_for_wow.rs` covers the tray's wait-for-WoW mode
(idle while the game is closed, final delivery at logout).

To type-check the Linux and Windows code paths from macOS without a C
cross-compiler, leave out TLS (its crypto library needs one):
`cargo clippy --no-default-features --target x86_64-pc-windows-gnu`.

## Limitations

- Sending by screenshot briefly shows a colored grid in the top-left corner
  and costs a short hitch while the image is saved. The screenshot format is
  switched to JPEG for that moment and restored afterwards. About 2.3 KB fit
  in one screenshot; longer messages take several. When this channel is off or
  fails, sending reloads the UI.
- Tool permission prompts cannot be approved over MCP. Approve the bridge
  with **Auto** or **Full access** if threads started from the game should
  run unattended.
- WoW fonts lack emoji and some scripts. Inline bold and italics are shown as
  colors (a font string cannot switch fonts mid-line), and rendered text is
  not selectable: use **Copy**.
- On macOS and Linux every live slot's `Data.lua` is a symlink to one shared
  file, so a snapshot is written once however many slots there are. If a game
  client does not follow the links (live updates never arrive), reinstall with
  `--copy-slots`. Windows always copies.
- Live updates via load-on-demand addons need to be verified in each game
  client. If a client caches addon files, updates still arrive with
  Sync/`/reload`.
