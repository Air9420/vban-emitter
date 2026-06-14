# VBAN Emitter for Windows

VBAN audio streamer — captures system audio via WASAPI loopback and streams it over LAN using the VBAN protocol.

## Features

- **WASAPI Loopback** capture (no Voicemeeter needed on sender)
- **Audio-clock driven** architecture (no timer, data-driven sending)
- **Bounded ring buffer** with drop-oldest policy (latency never grows unbounded)
- **f32→S16 automatic conversion** (WASAPI engine handles format conversion)
- **High process priority** for reduced scheduling jitter
- Minimal dependencies: `wasapi`, `clap`, `windows`

## Usage

```bash
# Default: send to 192.168.31.254:6980, stream name "air-mini"
vban_emitter.exe

# Custom target
vban_emitter.exe -i 192.168.31.100 -p 6980 -s mystream

# List audio devices
vban_emitter.exe --list
```

## Receiver Setup

On the receiving PC, use [Voicemeeter](https://vb-audio.com/Voicemeeter/) with VBAN:

1. Open VBAN panel (Menu → VBAN)
2. INCOMING STREAMS: set Stream Name, IP of sender, Port 6980
3. Enable ON

## Architecture

```
WASAPI Loopback (audio clock)
    ↓
Convert to S16
    ↓
Bounded ring buffer (drop-oldest)
    ↓
Send when enough data (data-driven, no timer)
    ↓
VBAN UDP packet → network
```

## Build

```bash
cargo build --release
```

## Tools

| Binary | Description |
|--------|-------------|
| `vban_emitter` | Main VBAN streamer |
| `vban_sine` | Pure sine wave test (no WASAPI, verifies VBAN format) |
| `debug_audio` | Debug tool: dumps raw WASAPI data in f32/i32 |

## Limitations

- Windows WASAPI loopback has inherent timing jitter (~2-5ms), limiting minimum stable buffer to ~256 samples (5.3ms)
- For sub-2ms latency, use ASIO drivers (not available for display audio)
- VBAN max 256 samples per frame (protocol limit)

## License

MIT
