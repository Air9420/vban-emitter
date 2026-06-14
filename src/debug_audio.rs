use wasapi::{deinitialize, get_default_device, initialize_mta, Direction, ShareMode};

fn main() {
    initialize_mta().ok();
    let dev = get_default_device(&Direction::Render).expect("No render device");
    let mut client = dev.get_iaudioclient().expect("No AudioClient");
    let fmt = client.get_mixformat().unwrap();
    let sr = fmt.get_samplespersec();
    let ch = fmt.get_nchannels();
    let bps = fmt.get_bitspersample() / 8;
    let bpf = fmt.get_blockalign();
    let sub = fmt.get_subformat().unwrap();

    println!("Format: {}Hz {}ch {}bps {:?} bpf={}", sr, ch, bps * 8, sub, bpf);

    client.initialize_client(&fmt, 0, &Direction::Capture, &ShareMode::Shared, false).unwrap();
    let cap = client.get_audiocaptureclient().unwrap();
    let evt = client.set_get_eventhandle().unwrap();
    client.start_stream().unwrap();

    println!("Listening... play some audio!\n");

    for _ in 0..5 {
        let _ = evt.wait_for_event(200);
        loop {
            let frames = match cap.get_next_nbr_frames() {
                Ok(Some(n)) if n > 0 => n as usize,
                _ => break,
            };
            let mut buf = vec![0u8; frames * bpf as usize];
            match cap.read_from_device(&mut buf) {
                Ok((read, _)) if read > 0 => {
                    let len = read as usize * bpf as usize;
                    println!("Got {} frames ({} bytes)", read, len);
                    // Print first 8 samples (32 bytes for 2ch f32)
                    let show = len.min(32);
                    print!("  Raw bytes: ");
                    for b in &buf[..show] {
                        print!("{:02x} ", b);
                    }
                    println!();
                    // Interpret as f32
                    print!("  As f32:    ");
                    for chunk in buf[..show].chunks_exact(4) {
                        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        print!("{:>12.6} ", v);
                    }
                    println!();
                    // Interpret as i32
                    print!("  As i32:    ");
                    for chunk in buf[..show].chunks_exact(4) {
                        let v = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        print!("{:>12} ", v);
                    }
                    println!();
                    println!();
                    break;
                }
                _ => break,
            }
        }
    }
    client.stop_stream().unwrap();
    deinitialize();
}
