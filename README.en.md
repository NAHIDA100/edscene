# edscene · Integrated Industries Broadcast Terminal

A **non-commercial fan project** for Arknights: Endfield. It watches your
system's logon, unlock, lock, screensaver, battery-saver, low-battery and AC
adapter events and plays a voice clip at the moment each one happens — like a
base announcement, like a system sound.

The UI reproduces the game's settings panel: 谷地黄 `#F5FF01` accent,
translucent bands, 45° hatch, white pill controls, `//设置` header,
`恢复默认` in the corner.

> **No game assets are included.** The repository contains no audio, no images,
> no fonts and no installers. The app stays silent until you point it at a
> sound pack of your own. Not affiliated with, endorsed by, or connected to
> Hypergryph / Mountain Contour / Yostar.

Full documentation is in [`README.md`](README.md); this file is the short route.

## Quick start

```powershell
# Windows
edscene --selftest --sounds "D:\path\to\clips"   # resolve all eight presets
edscene --install-service                          # autostart, one UAC prompt
edscene                                            # settings window
```

```sh
# Linux
edscene --selftest --sounds ~/clips
edscene --agent &
```

## Presets

Eight events, each named after its factory clip (`解锁返回桌面.mp3`, `锁屏.mp3`,
`电池电量低于5%.mp3`, …). Matching is by file **stem**, so `锁屏.wav` works as
well as `锁屏.mp3`, and the real container is sniffed from the file header —
which matters, because voice packs exported from the game are usually
AAC-in-MP4 streams wearing a `.mp3` suffix.

Supported: MP3, M4A/MP4-AAC, ADTS-AAC, WAV, FLAC, Ogg Vorbis, AIFF.
Explicitly unsupported (the UI says so instead of failing silently): Opus, WMA.

## Replacing a clip, restoring the factory state

Resolution order, highest first: the app's override folder → a per-event
absolute path in the config → the factory name inside your sound directory.
Swapping a file in place takes effect on the next broadcast, with no restart.
`恢复` reverts one event; `恢复默认` (press twice) reverts everything. Retired
files are moved to `sounds/.disabled/`, never deleted.

## Architecture

```
eds-core    EventKind, the eight presets, the pure signal→broadcast state
            machine, config, sound resolution, image maths   (45 unit tests)
eds-windows message-only window + WTS session changes + power polling,
            the LocalSystem service, cross-session agent launch, named pipes
eds-linux   login1 / UPower / ScreenSaver over D-Bus, systemd user unit,
            XDG autostart, wallpaper discovery                (pure Rust)
eds-app     the binary: CLI, audio engine, Slint UI
```

The service is the only event originator; the agent in your session owns the
audio device and contains no OS watchers, so a double broadcast is impossible
by construction. Nothing plays while idle: the output stream is released five
seconds after the last broadcast, so the endpoint stays free.

## Build

```sh
cargo build --release
cargo test --workspace
```

Windows needs a MinGW for `dlltool`, and Chinese networks need a crates.io
mirror — see [`docs/dev-setup-windows.md`](docs/dev-setup-windows.md).

## Licence

Code MIT (`LICENSE-code`). Skin, copy and preset names CC BY-NC-ND 4.0
(`LICENSE-content`). Disclaimer and third-party notices in `NOTICE`.
