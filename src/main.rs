use std::collections::VecDeque;
use std::net::UdpSocket;
use std::time::Instant;

use clap::Parser;
use wasapi::{
    deinitialize, get_default_device, initialize_mta, DeviceCollection, Direction, SampleType,
    ShareMode, WaveFormat,
};

#[derive(Parser, Debug)]
#[command(name = "vban_emitter")]
struct Args {
    #[arg(short = 'i', long, default_value = "192.168.31.254")]
    ip: String,
    #[arg(short, long, default_value_t = 6980)]
    port: u16,
    #[arg(short, long, default_value = "air-mini")]
    stream: String,
    #[arg(short = 'x', long, default_value_t = 0)]
    bufsize: usize,
    #[arg(long, default_value_t = 32768)]
    ring_capacity: usize,
    #[arg(short, long)]
    list: bool,
    /// Force f32 conversion (default: let WASAPI convert to S16)
    #[arg(long)]
    force_float: bool,
}

const VBAN_SR: [u32; 21] = [
    6000, 12000, 24000, 48000, 96000, 192000, 384000, 8000, 16000, 32000, 64000, 128000,
    256000, 512000, 11025, 22050, 44100, 88200, 176400, 352800, 705600,
];

fn get_sr_index(rate: u32) -> u8 {
    VBAN_SR.iter().position(|&r| r == rate).unwrap_or(3) as u8
}

fn build_packet(name: &str, sr: u8, ch: u8, fc: u32, ns: u16, data: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; 28 + data.len()];
    p[0] = b'V'; p[1] = b'B'; p[2] = b'A'; p[3] = b'N';
    p[4] = sr & 0x1F;
    p[5] = (ns as u8).wrapping_sub(1);
    p[6] = ch.wrapping_sub(1);
    p[7] = 0x01;
    let n = name.as_bytes();
    let len = n.len().min(15);
    p[8..8 + len].copy_from_slice(&n[..len]);
    p[24..28].copy_from_slice(&fc.to_le_bytes());
    p[28..].copy_from_slice(data);
    p
}

fn list_devices() {
    initialize_mta().ok();
    if let Ok(c) = DeviceCollection::new(&Direction::Render) {
        for d in c.into_iter().flatten() {
            println!("  {}", d.get_friendlyname().unwrap_or_default());
        }
    }
    deinitialize();
}

fn main() {
    let args = Args::parse();
    if args.list { list_devices(); return; }

    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS};
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }

    initialize_mta().ok();
    let dev = get_default_device(&Direction::Render).expect("No render device");
    let dev_name = dev.get_friendlyname().unwrap_or_default();
    let mut client = dev.get_iaudioclient().expect("No AudioClient");

    let native_fmt = client.get_mixformat().unwrap();
    let sr = native_fmt.get_samplespersec() as usize;
    let ch = native_fmt.get_nchannels() as usize;

    // Choose capture format: S16 (let WASAPI convert) or native f32
    let (capture_fmt, src_bps, is_float) = if args.force_float {
        (native_fmt.clone(), 4usize, true)
    } else {
        // Request S16 from WASAPI — let the engine do the conversion
        let fmt = WaveFormat::new(16, 16, &SampleType::Int, sr, ch, None);
        (fmt, 2usize, false)
    };

    let src_bpf = ch * src_bps;

    // Try to initialize with S16 first, fall back to native if it fails
    let init_result = client.initialize_client(&capture_fmt, 0, &Direction::Capture, &ShareMode::Shared, true);
    if init_result.is_err() && !args.force_float {
        println!("S16 format not supported, falling back to native float");
        client = dev.get_iaudioclient().unwrap();
        client.initialize_client(&native_fmt, 0, &Direction::Capture, &ShareMode::Shared, false).unwrap();
    }

    let buffer_frames = client.get_bufferframecount().unwrap() as usize;
    let cap = client.get_audiocaptureclient().unwrap();
    let evt = client.set_get_eventhandle().unwrap();
    client.start_stream().unwrap();

    let vban_bpf = ch * 2;
    let vban_frames = if args.bufsize > 0 { args.bufsize.min(256) } else { buffer_frames.min(256) };
    let chunk = vban_frames * vban_bpf;
    let sr_idx = get_sr_index(sr as u32);

    println!("=== VBAN Emitter (Audio-Clock) ===");
    println!("Target:   {}:{}", args.ip, args.port);
    println!("Stream:   {}", args.stream);
    println!("Device:   {} [Loopback]", dev_name);
    println!("Audio:    {}Hz / {}ch / {}bps ({})", sr, ch, src_bps * 8, if is_float { "float" } else { "int" });
    println!("WASAPI:   {} frames ({:.1}ms)", buffer_frames, buffer_frames as f64 / sr as f64 * 1000.0);
    println!("VBAN:     {} frames ({:.1}ms)", vban_frames, vban_frames as f64 / sr as f64 * 1000.0);
    println!();

    let dest = format!("{}:{}", args.ip, args.port);
    let sock = UdpSocket::bind("0.0.0.0:0").unwrap();

    let mut ring: VecDeque<u8> = VecDeque::with_capacity(args.ring_capacity);
    let mut fc: u32 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_drops: u64 = 0;
    let t0 = Instant::now();
    let mut last_report = t0;
    let mut convert_buf = Vec::with_capacity(8192);

    println!("Streaming... (Ctrl+C to stop)\n");

    loop {
        let _ = evt.wait_for_event(100);

        loop {
            let frames = match cap.get_next_nbr_frames() {
                Ok(Some(n)) if n > 0 => n as usize,
                _ => break,
            };
            let mut buf = vec![0u8; frames * src_bpf];
            match cap.read_from_device(&mut buf) {
                Ok((read, flags)) if read > 0 && !flags.silent => {
                    let len = read as usize * src_bpf;
                    if is_float {
                        // f32 → i16
                        convert_buf.clear();
                        convert_buf.reserve(len / 2);
                        for chunk in buf[..len].chunks_exact(4) {
                            let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                            let v = (s.max(-1.0).min(1.0) * 32767.0) as i16;
                            convert_buf.extend_from_slice(&v.to_le_bytes());
                        }
                        while ring.len() + convert_buf.len() > args.ring_capacity {
                            let drain = chunk.min(ring.len());
                            ring.drain(..drain);
                            total_drops += 1;
                        }
                        ring.extend(convert_buf.iter());
                    } else {
                        // Already S16, direct copy
                        while ring.len() + len > args.ring_capacity {
                            let drain = chunk.min(ring.len());
                            ring.drain(..drain);
                            total_drops += 1;
                        }
                        ring.extend(&buf[..len]);
                    }
                }
                _ => break,
            }
        }

        while ring.len() >= chunk {
            let pcm: Vec<u8> = ring.drain(..chunk).collect();
            let nb = (pcm.len() / vban_bpf) as u16;
            let pkt = build_packet(&args.stream, sr_idx, ch as u8, fc, nb, &pcm);
            let _ = sock.send_to(&pkt, &dest);
            fc += 1;
            total_bytes += pcm.len() as u64;
        }
        // Residual data stays in ring — no silence padding, no extra packets

        let now = Instant::now();
        if now.duration_since(last_report).as_secs_f64() >= 2.0 {
            let elapsed = now.duration_since(t0).as_secs_f64();
            println!(
                "  #{:>8} {:>6}KB {:>7.0}KB/s {:>6.1}pkt/s ring={:<6} drop={:<4} {:>6.1}s",
                fc, total_bytes / 1024, total_bytes as f64 / elapsed / 1024.0,
                fc as f64 / elapsed, ring.len(), total_drops, elapsed
            );
            last_report = now;
        }
    }
}
