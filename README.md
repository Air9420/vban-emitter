# VBAN Emitter for Windows

VBAN audio streamer — captures system audio via WASAPI loopback and streams it over LAN using the VBAN protocol.

## Features

- **System tray icon** with real-time status display
- **No console window** — runs silently in background
- **WASAPI Loopback** capture (no Voicemeeter needed on sender)
- **Audio-clock driven** architecture (data-driven, no timer)
- **Bounded ring buffer** with drop-oldest policy
- **Copy Info to clipboard** for debugging
- **Separate audio/UI threads** with panic-safe error handling

## Usage

```bash
# Default: tray mode, send to 192.168.31.254:6980
vban_emitter.exe

# Custom target
vban_emitter.exe -i 192.168.31.100 -p 6980 -s mystream

# Headless mode (no tray, console output)
vban_emitter.exe --no-tray

# List audio devices
vban_emitter.exe --list
```

## Tray Icon

| Action | Effect |
|--------|--------|
| Right-click | Menu: real-time status, Copy Info, Quit |
| Left-click | Copy full status to clipboard |

## Architecture

```
WASAPI Loopback (audio clock, background thread)
    ↓
Convert to S16 (if needed)
    ↓
Bounded ring buffer (drop-oldest)
    ↓
Data-driven send → VBAN UDP
    ↓
Status → mpsc channel → Tray UI thread
```

## Build

```bash
cargo build --release
```

## Tools

| Binary | Description |
|--------|-------------|
| `vban_emitter` | Main VBAN streamer with tray icon |
| `vban_sine` | Pure sine wave test (verifies VBAN format) |
| `debug_audio` | Dumps raw WASAPI data for debugging |

## Receiver Setup

Use [Voicemeeter](https://vb-audio.com/Voicemeeter/) with VBAN:

1. Open VBAN panel (Menu -> VBAN)
2. INCOMING STREAMS: set Stream Name, IP of sender, Port 6980
3. Enable ON

## Limitations

- Windows WASAPI loopback has inherent timing jitter (~2-5ms)
- Minimum stable buffer: 256 samples (5.3ms @ 48kHz)
- VBAN protocol max: 256 samples per frame
- For sub-2ms latency, use ASIO drivers

## License

MIT
