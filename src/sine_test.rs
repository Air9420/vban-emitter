use std::f32::consts::PI;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

/// VBAN sine wave test — pure tone, no WASAPI, to verify packet format
fn main() {
    let ip = "192.168.31.254";
    let port = 6980u16;
    let stream = "air-mini";
    let sr = 48000u32;
    let ch = 2u16;
    let frames = 256u16;
    let freq = 440.0f32;

    let dest = format!("{}:{}", ip, port);
    let sock = UdpSocket::bind("0.0.0.0:0").unwrap();

    let sr_idx = 3u8; // 48000Hz
    let bpf = ch as usize * 2; // bytes per frame (S16)
    let chunk = frames as usize * bpf;
    let interval = Duration::from_secs_f64(frames as f64 / sr as f64);

    println!("=== VBAN Sine Test ===");
    println!("Sending {}Hz tone to {}:{}", freq, ip, port);
    println!("{}Hz / {}ch / S16 / {} samples/packet", sr, ch, frames);
    println!();

    let mut fc: u32 = 0;
    let mut phase: f32 = 0.0;
    let t0 = Instant::now();
    let mut next_send = t0;

    loop {
        // Generate one frame of sine wave
        let mut pcm = vec![0u8; chunk];
        for i in 0..frames as usize {
            let sample = (phase * 2.0 * PI).sin() * 0.8; // 80% amplitude
            let i16_val = (sample * 32767.0) as i16;
            let bytes = i16_val.to_le_bytes();
            // Stereo: same sample to both channels
            let offset = i * 4; // 2 channels * 2 bytes
            pcm[offset] = bytes[0];
            pcm[offset + 1] = bytes[1];
            pcm[offset + 2] = bytes[0];
            pcm[offset + 3] = bytes[1];
            phase += freq / sr as f32;
            if phase >= 1.0 {
                phase -= 1.0;
            }
        }

        // Build VBAN packet
        let mut pkt = vec![0u8; 28 + chunk];
        pkt[0] = b'V';
        pkt[1] = b'B';
        pkt[2] = b'A';
        pkt[3] = b'N';
        pkt[4] = sr_idx & 0x1F;
        pkt[5] = (frames as u8).wrapping_sub(1);
        pkt[6] = (ch as u8).wrapping_sub(1);
        pkt[7] = 0x01; // PCM | 16-bit
        let name = stream.as_bytes();
        pkt[8..8 + name.len()].copy_from_slice(name);
        pkt[24..28].copy_from_slice(&fc.to_le_bytes());
        pkt[28..].copy_from_slice(&pcm);

        sock.send_to(&pkt, &dest).unwrap();
        fc += 1;

        // Report every 2s
        let now = Instant::now();
        if now.duration_since(t0).as_secs_f64() >= 2.0 && fc % 100 == 0 {
            println!("  #{} sent", fc);
        }

        // Pace: send at exactly the right rate
        next_send += interval;
        let sleep = next_send.saturating_duration_since(Instant::now());
        if sleep > Duration::from_micros(50) {
            std::thread::sleep(sleep);
        }
    }
}
