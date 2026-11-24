# tidalfast API

Everything you need to make **themes** and **plugins** for tidalfast.

- [Themes](#themes): colours in a small text file, no code. Five minutes.
- [Plugins](#plugins): scripts that add buttons, menus and settings, react to the music and change how tidalfast behaves, in a sandbox.
- [Security](#security): what plugins can and cannot do, and how you stay in control.

Examples to copy from are in [`examples/`](examples/).

---

## Themes

A theme is one `.toml` file. It cannot run anything; it only changes colours.

### Where it goes

| System  | Folder                                              |
|---------|-----------------------------------------------------|
| Linux   | `~/.config/tidalfast/themes/`                       |
| Windows | `%APPDATA%\tidalfast\themes\`                       |
| macOS   | `~/Library/Application Support/tidalfast/themes/`   |

*Settings → Appearance → Theme → Open themes folder* opens it. After adding or editing a file press *Reload* there, then pick the theme. The file name (without `.toml`) is the theme's id: small letters, digits, `-` and `_` only.

### Format

```toml
name = "Midnight"                 # required, shown in the picker
author = "you"                    # optional
description = "Cool blue on navy" # optional, shown as a tooltip
follow_cover = false              # optional, see below

[colors]
accent = "#8ab4f8"                # required: buttons, highlights, active things
panel = "#0f1220"                 # optional: the tint of the glass panels
wallpaper = ["#1a2a6c", "#2b1a5e", "#0b3a5c"]   # optional: 1 to 3 colours of the background
light = 0.0                       # optional: 0 dark ... 1 bright backdrop
```

| Key | Meaning | If left out |
|-----|---------|-------------|
| `name` | Name in the picker (max. 40 characters). | The theme is refused. |
| `follow_cover` | `true`: the colours follow the cover of the playing song like the built-in look does; the colours below are used when there is nothing to follow (nothing playing, grey cover). `false`: always these colours, and no blurred cover behind the app. | `false` |
| `colors.accent` | The accent colour. | The theme is refused. |
| `colors.panel` | Tint of the glass panels. | A very dark version of the accent's hue. |
| `colors.wallpaper` | The colours of the soft background (and of the "Liked Songs" tile). With one colour all three spots use it, with two the second is used twice. | Derived from the accent. |
| `colors.light` | How bright the backdrop is. Bright backdrops get more opaque panels so text stays readable. | Worked out from the wallpaper colours. |

Colours are written `#rrggbb` or `#rgb`.

### What is corrected for you

The text in tidalfast is light on dark glass, so themes cannot make the panels light. To keep everything readable:

- the **panel** colour is darkened if it is lighter than 20 % lightness;
- the **accent** is lightened until it has a contrast of at least 4.5 against the glass (an accent is also used as text and as button fill).

A file that cannot be used (a typo in a colour, a missing `accent`, …) is listed under the theme picker with the reason.

### Tips

- Start from [`examples/themes/midnight.toml`](examples/themes/midnight.toml) or `sunset.toml`.
- `follow_cover = true` with your own `accent` and `wallpaper` gives you a dynamic theme with a nice fallback.
- Share a theme by sending the file. There is nothing to install except putting it in the folder.

---

## Plugins

A plugin is a folder with two files:

```
plugins/
  my-plugin/
    plugin.toml     what it is and what it asks for
    main.rhai       the script
```

Plugins are written in [Rhai](https://rhai.rs), a small scripting language that looks like JavaScript/Rust. The language is sandboxed on purpose: a script has no way to touch files, programs or the network except through the functions listed here, and every one of those checks the plugin's permissions.

### Where it goes

| System  | Folder                                               |
|---------|------------------------------------------------------|
| Linux   | `~/.config/tidalfast/plugins/`                       |
| Windows | `%APPDATA%\tidalfast\plugins\`                       |
| macOS   | `~/Library/Application Support/tidalfast/plugins/`   |

The folder's name must be the plugin's `id`. In *Settings → Plugins* use *Open plugins folder*, then *Reload*. A plugin **never runs until the user has seen what it asks for and allowed it** (see [Security](#security)).

### A first plugin

`plugins/hello/plugin.toml`:

```toml
id = "hello"
name = "Hello"
version = "1.0"
api = 1
permissions = ["ui.buttons", "ui.toast"]
```

`plugins/hello/main.rhai`:

```rust
ui::button(#{ id: "wave", label: "Say hello", icon: "sparkle", place: "header" });

fn on_action(id, data) {
    if id == "wave" {
        ui::toast("Hello!");
    }
}
```

Open *Settings → Plugins*, press *Reload*, *Turn on…* and allow it. A sparkle button appears next to the settings button. Click it.

### plugin.toml

```toml
id = "my-plugin"              # required: 1-40 small letters, digits, - and _; must equal the folder name
name = "My plugin"            # required
version = "1.0"               # required (free text)
api = 1                       # required: the interface version, currently 1
author = "you"                # optional
description = "What it does"  # optional (max. 300 characters)
homepage = "https://..."      # optional
permissions = ["player.read", "ui.toast"]   # what it needs; see the table below

[network]                     # only with the permission "network"
hosts = ["api.example.com"]   # exact server names, at most 8
```

A manifest with an unknown permission, an `api` the app does not speak, or network hosts without the `network` permission (or the other way round) is refused with a message.

### How a script runs

The script runs **once**, when the plugin starts. Use that to register buttons, menu entries and settings. After that the app calls the **handler functions** you have defined, when something happens:

| Function | Called when | Arguments |
|----------|-------------|-----------|
| `on_load()` | Once, right after the start (and after everything registered at the top is in place). | none |
| `on_track(track)` | The song changed. Also once at start if something plays. | a [track](#the-track-map) |
| `on_playing(playing)` | Playback started (`true`) or paused/stopped (`false`). | bool |
| `on_tick()` | About once a second, while the plugin runs. | none |
| `on_action(id, data)` | One of the plugin's buttons, song-menu entries or settings buttons was used. | the id; `data` is `#{ track: <track> }` for song-menu entries, `()` otherwise |
| `on_setting(id, value)` | The user changed one of the plugin's settings. `value` is the accepted (checked, clamped) value; for a settings button `true`. | id, value |
| `on_unload()` | The plugin is being turned off. | none |
| `hook_next()`, `hook_previous()`, `hook_play(track)` | See [Hooks](#hooks). | |

Only define the ones you need.

### Rhai in two minutes

```rust
let x = 5;                      // variables
const NAME = "tidalfast";       // constants
let list = [1, 2, 3];           // arrays
let map = #{ a: 1, b: "two" };  // maps (objects); map.a or map["a"]
let nothing = ();               // "no value" (like null)
let text = "Song: " + track.title + " (" + track.duration + " s)";

if x > 3 { ... } else { ... }
for item in list { ... }
for i in 0..10 { ... }          // 0 to 9
while cond { ... }

fn double(n) { n * 2 }          // the last expression is the result
let y = storage::get("k") ?? 0; // ?? gives a default when the left side is ()
```

Things that are different from what you may expect:

- **Functions cannot see variables defined outside them.** To keep something between calls use [`storage`](#storage) (kept on disk), [`state`](#state) (kept while tidalfast runs) or [`settings::get`](#settings).
- `import` and `eval` are switched off. All code goes in `main.rhai`.
- Integers (`1`) and floats (`1.0`) are different types. Mixing them in a sum usually works, but when something complains about a type, convert with `to_float(x)` / `to_int(x)`.

The [Rhai book](https://rhai.rs/book/) describes the whole language. Only the parts that need no files or system access are available.

### Permissions

| Permission | Risk | What the plugin may do |
|------------|------|------------------------|
| `player.read` | low | See what is playing (title, artist, album, position) and whether it is playing. |
| `player.control` | medium | Play, pause, skip, seek, change the volume. |
| `ui.toast` | low | Show short notifications (prefixed with the plugin's name). |
| `ui.buttons` | low | Add buttons to the header, the player bar and the full-screen player. |
| `ui.menus` | low | Add entries to the song (right-click) menu. |
| `ui.settings` | low | Add its own settings pages and entries to the built-in pages. |
| `ui.modify` | medium | Hide built-in parts of the interface. |
| `hooks` | high | Be asked before a skip or a song start, and cancel it. |
| `storage` | low | Keep small amounts of its own data (512 KB). |
| `network` | high | Talk to the servers listed under `[network]` (HTTPS GET only). |
| `clipboard` | medium | Put text into the clipboard. |
| `open_url` | medium | Open `https://` links in the browser. |

A function called without its permission stops with the message `permission denied: this plugin was not given “…”`. Ask only for what you use; people read the list before they allow a plugin.

### Reference

Everything is called as `module::function(...)`. The *Needs* column is the permission.

#### app

| Function | Needs | Description |
|----------|-------|-------------|
| `app::version()` | – | tidalfast's version, e.g. `"0.7.0"`. |
| `app::api_version()` | – | The interface version, `1`. |
| `app::features()` | – | Array of the names of the parts that can be hidden. |
| `app::hide(part)` | `ui.modify` | Hide a built-in part of the interface, see [Hiding parts](#hiding-parts). |
| `app::show(part)` | `ui.modify` | Show it again. |

#### player

| Function | Needs | Description |
|----------|-------|-------------|
| `player::current()` | `player.read` | The [track](#the-track-map) that is loaded, or `()`. |
| `player::position()` | `player.read` | Position in the song in seconds (float). |
| `player::is_playing()` | `player.read` | `true` while music plays. |
| `player::volume()` | `player.read` | Volume, `0.0` to `1.0`. |
| `player::play()` / `pause()` / `toggle()` | `player.control` | |
| `player::next()` / `previous()` | `player.control` | Same as the buttons (hooks of plugins apply). |
| `player::seek(seconds)` | `player.control` | |
| `player::set_volume(v)` | `player.control` | `0.0` to `1.0`. |

At most 5 player commands per handler call.

#### ui

| Function | Needs | Description |
|----------|-------|-------------|
| `ui::toast(text)` | `ui.toast` | A short notification, shown as `Plugin name: text`. At most 3 per handler call. |
| `ui::button(map)` | `ui.buttons` | Add (or replace) a button. Keys below. Up to 8 per plugin. |
| `ui::menu_item(map)` | `ui.menus` | Add (or replace) an entry to the song menu. Up to 16. |

Button keys: `id` (required, 1-40 of `a-z 0-9 - _ .`), `label` (tooltip fallback), `tooltip`, `icon` (see [icons](#icons)), `place`: `"header"` (next to the settings button, default), `"player"` (player bar) or `"now"` (top bar of the full-screen player). Buttons are icon-only; the tooltip shows on hover. When clicked, `on_action(id, ())` is called.

Menu item keys: `id`, `label` (required), `icon`. When clicked, `on_action(id, #{ track: <track> })` is called with the song the menu was opened on.

#### settings

Settings pages let people configure your plugin. Values are remembered by tidalfast (no `storage` permission needed) and come back the next time the plugin starts: the functions that create an entry return the **current** value.

```rust
settings::category("mine", "My plugin", "gear");      // a page of its own (id, title, icon)
settings::heading("mine", "Notifications");            // (page, text)
settings::label("mine", "info", "Songs so far: 0");    // (page, id, text), update with set_label
let on   = settings::toggle("mine", "loud", "Be loud", "Help text", true);          // (page, id, label, help, default) -> bool
let name = settings::text("mine", "name", "Name", "Help text", "default");           // -> string
let vol  = settings::number("mine", "vol", "Volume", "Help text", 5.0, 0.0, 10.0, 1.0); // (.., default, min, max, step) -> float
let mode = settings::choice("mine", "mode", "Mode", "Help", ["fast", "slow"], "fast"); // (.., options, default) -> string
settings::button("mine", "reset", "Reset", "Help text");   // fires on_setting("reset", true)
```

The first argument is the page: either one you created with `settings::category`, or a **built-in page**, in which case the entry is added at the end of that page under "From plugins": `"playback"`, `"equalizer"`, `"appearance"`, `"discord"`, `"overlay"`, `"system"`, `"account"`.

| Function | Needs | Description |
|----------|-------|-------------|
| `settings::category(id, title, icon)` | `ui.settings` | Create a page (up to 6). Shown in the settings menu. |
| `settings::heading(page, text)` | `ui.settings` | A heading. |
| `settings::label(page, id, text)` | `ui.settings` | A line of text. |
| `settings::toggle(...)`, `text(...)`, `number(...)`, `choice(...)` | `ui.settings` | Input entries (see above). Up to 80 entries per plugin; ids must be unique. |
| `settings::button(page, id, label, help)` | `ui.settings` | A button. |
| `settings::set_label(id, text)` | `ui.settings` | Change the text of an entry (handy for status lines). |
| `settings::get(id)` | `ui.settings` | The current value of an entry (`()` for headings, labels and buttons). Use this inside handler functions. |

When the user changes something, `on_setting(id, value)` is called with the value that was accepted: numbers are clamped to `min`/`max`, a choice is always one of the options, text is cleaned and cut to 500 characters.

#### storage

Small amounts of data on disk, private to the plugin (`plugin-data/<id>.json` next to the settings). 512 KB in total.

| Function | Needs | Description |
|----------|-------|-------------|
| `storage::get(key)` | `storage` | The stored value, or `()`. |
| `storage::set(key, value)` | `storage` | Store numbers, strings, bools, arrays and maps. Keys have 1-64 characters. |
| `storage::remove(key)` | `storage` | |
| `storage::keys()` | `storage` | Array of keys. |

#### state

Like storage, but only in memory while tidalfast runs, and without a permission. Up to 256 values.

| Function | Description |
|----------|-------------|
| `state::get(key)` | The value or `()`. |
| `state::set(key, value)` | |

#### http

Needs `network` and the server listed under `[network] hosts`. Only `https://`, only the standard port, no passwords in the address, redirects only to other approved hosts, answers up to 1 MB, 8 seconds, 30 requests per minute. Addresses that lead to this computer or to the home network are refused.

| Function | Description |
|----------|-------------|
| `http::get(url)` | A map `#{ status, ok, body }`. |
| `http::get_json(url)` | The parsed JSON (fails on a status outside 200-299 or invalid JSON). |

There is no POST and you cannot set headers. A call takes as long as the server needs, so use it in handlers like `on_track`, not in a tight loop. While it waits, only your plugin waits.

```rust
// permissions = ["network", "ui.toast"], [network] hosts = ["api.example.com"]
fn on_track(track) {
    let r = http::get_json("https://api.example.com/info?q=" + track.title);
    ui::toast(r.summary);
}
```

#### system

| Function | Needs | Description |
|----------|-------|-------------|
| `system::copy(text)` | `clipboard` | Put text into the clipboard. |
| `system::open_url(url)` | `open_url` | Open an `https://` link in the browser (one per 3 seconds). |

At most one clipboard or link action per handler call.

#### util

| Function | Description |
|----------|-------------|
| `util::parse_json(text)` | JSON text to a value. |
| `util::to_json(value)` | A value to JSON text. |
| `util::now()` | Seconds since 1970 (float). |

`print(x)` and `debug(x)` write to the plugin's *Messages* in *Settings → Plugins*.

### The track map

`on_track`, `hook_play`, `player::current()` and the song menu give you a map with:

| Key | Type | |
|-----|------|-|
| `id` | int | TIDAL's id of the song |
| `title` | string | |
| `artist` | string | The main artist(s) as one text |
| `artists` | array of strings | |
| `album` | string | Empty if unknown |
| `duration` | int | Seconds |
| `explicit` | bool | |
| `cover` | string | URL of the cover picture (a TIDAL image URL), empty if none |

### Hooks

A hook lets a plugin **say no** to something the user did. Needs `hooks`.

| Function | Asked when |
|----------|------------|
| `hook_next()` | The user (or a plugin) presses *next*. Not for a song that simply ends. |
| `hook_previous()` | The user presses *previous*. |
| `hook_play(track)` | The user starts a song or a playlist (the first song is passed). |

Return `false` to cancel; anything else (or an error, or taking longer than 100 ms) means yes. The app waits at most 150 ms for each plugin, so keep hooks quick.

```rust
fn hook_next() {
    !settings::get("block_skip")
}
```

### Hiding parts

With `ui.modify` a plugin can hide built-in parts of the interface, e.g. to build a "focus mode". Hiding is undone when the plugin is turned off or stops.

| Part | What it hides |
|------|---------------|
| `lyrics` | The Lyrics tab of the full-screen player |
| `credits` | The Credits tab of the full-screen player |
| `queue` | The queue button in the player bar |
| `like` | The heart button in the player bar |
| `volume` | The volume control in the player bar |
| `shuffle` | The shuffle button |
| `repeat` | The repeat button |
| `search` | The search field in the header |
| `settings.equalizer` | The Equalizer settings page |
| `settings.discord` | The Discord settings page |
| `settings.overlay` | The Overlay settings page |
| `settings.account` | The Account settings page |

Settings → Plugins is never hideable, so a plugin can not lock you out of turning it off.

### Icons

Icons are named; an unknown name shows a puzzle piece.

`star`, `heart`, `plus`, `minus`, `gear`, `download`, `upload`, `music`, `play`, `pause`, `skip-forward`, `skip-back`, `link`, `copy`, `trash`, `info`, `warning`, `check`, `x`, `search`, `list`, `clock`, `moon`, `sun`, `bell`, `globe`, `code`, `palette`, `share`, `bookmark`, `tag`, `timer`, `shuffle`, `repeat`, `microphone`, `headphones`, `speaker`, `lightning`, `sparkle`, `eye`, `eye-slash`, `pencil`, `folder`, `chart`

### Limits

These keep a misbehaving plugin from slowing tidalfast down. When a limit is hit, the current call stops with an error shown in the plugin's *Messages*; the plugin keeps running. A plugin that fails more than 20 times in a minute is stopped (*Restart* in the settings).

| Limit | Value |
|-------|-------|
| Size of `main.rhai` / `plugin.toml` | 256 KB / 16 KB |
| Work per call | 1,000,000 operations |
| Time per call | 400 ms (start: 1.5 s, hooks: 100 ms) |
| Call depth | 24 |
| Text length / array size / map size | 100,000 chars / 5,000 / 2,000 |
| Storage | 512 KB; state: 256 values |
| Buttons / menu entries / settings pages / entries | 8 / 16 / 6 / 80 |
| Per handler call | 3 notifications, 5 player commands, 1 clipboard or link action |
| Network | 30 requests per minute, 8 s each, 1 MB answer |

### Debugging

- *Settings → Plugins → Messages* of a plugin shows what it printed with `print()` and every error with the function it happened in.
- A syntax error in the script is shown as "Stopped: the script has an error …" with the position.
- Turn on **Developer mode** in *Settings → Plugins* while writing: then edits to `main.rhai` take effect after *Reload* without asking for approval again. (A plugin that asks for **more permissions** always needs approval.)
- Start tidalfast with `--no-plugins` to run without any plugin, e.g. if one misbehaves.

### Checklist for plugin authors

- Ask only for the permissions you use. Every "high" one makes people think twice.
- Write what the plugin does in `description`, and say which servers you contact and why.
- Keep handlers short. Do slow things (network) rarely.
- Keep your plugin working when `player::current()` is `()` (nothing playing).
- Never assume a notification arrives: the user can turn plugins off at any time.

---

## Security

Plugins are code from other people, so tidalfast treats them as untrusted.

### What a plugin can never do

- Read or write your files (except its own small storage), start programs, read environment variables or use the shell. The scripting language has no such functions, and `import`/`eval` are switched off.
- See your TIDAL login, tokens, account name, email or playlists. A plugin only ever gets the title, artist, album, duration and cover address of the current song, and the player's position and volume.
- Talk to the internet, unless it has the `network` permission, and then only to the HTTPS servers you saw in the approval dialog. It cannot reach this computer (`localhost`) or your home network, and it cannot follow a redirect to another server.
- Load or run native code (no DLLs, no `.so` files). A plugin is text.
- Freeze the app: each plugin runs in its own thread with limits on work and time.
- Pose as the app: its notifications are prefixed with its name, and its settings pages are labelled "from the plugin …".

### How you stay in control

- **Nothing runs without approval.** New plugins are off. Turning one on shows what it asks for in plain words, with risk colours, the servers it may reach and a fingerprint of its files.
- **Changes need approval again.** The approval is tied to the fingerprint (SHA-256 of `plugin.toml` and `main.rhai`). If the files change, or the plugin asks for more, it stops until you look at it again. *Developer mode* skips this only for edits to the script, never for new permissions.
- **Master switch.** *Settings → Plugins → Allow plugins* turns every plugin off at once. `tidalfast --no-plugins` starts without them.
- **Messages.** Everything a plugin prints or fails with is visible in its card.
- **Plugins are not installed from the internet by the app.** You put the folder there yourself; tidalfast never downloads plugins.

### What you still have to decide

Approving a plugin means trusting its author with exactly what the list says:

- `player.control` and `hooks` can change what plays and block buttons.
- `clipboard` and `open_url` can put text in your clipboard or open a web page (one at a time, and tidalfast shows a note when it happens).
- `network` lets the plugin send what it knows (the current song, its own settings) to the listed servers.
- `ui.modify` can hide parts of the interface.

Read the script if you can. It is a short text file. Only allow plugins from people you trust, and be careful with the "high" permissions.

### Reporting a problem

If you find a way out of the sandbox, please report it at <https://github.com/wildesPepega/tidalfast/issues> (or privately, if you prefer, via the maintainer's GitHub profile).

---

## Versions

The interface version is `api = 1`. Plugins for another version are refused with a message. Within version 1, new functions can be added, but existing ones will keep working. Anything that would break a plugin gets a new `api` number.

Themes have no version: unknown keys are ignored, so a theme written today keeps working.
