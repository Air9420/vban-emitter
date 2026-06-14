#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

use clap::Parser;
use muda::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
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
    #[arg(long)]
    no_tray: bool,
    #[arg(short, long)]
    list: bool,
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
    p[0] = b'V';
    p[1] = b'B';
    p[2] = b'A';
    p[3] = b'N';
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

fn create_icon() -> Icon {
    let mut rgba = vec![0u8; 16 * 16 * 4];
    for y in 0..16u32 {
        for x in 0..16u32 {
            let idx = (y * 16 + x) as usize * 4;
            rgba[idx] = 0x22;
            rgba[idx + 1] = 0x66;
            rgba[idx + 2] = 0xCC;
            rgba[idx + 3] = 0xFF;
            let cx = x as i32 - 8;
            let cy = y as i32 - 4;
            if cy >= 0 && cy < 10 {
                let left_edge = -cy / 2 + 1;
                let right_edge = cy / 2 - 1;
                if cx >= left_edge && cx <= right_edge && cy > 2 {
                    rgba[idx] = 0xFF;
                    rgba[idx + 1] = 0xFF;
                    rgba[idx + 2] = 0xFF;
                }
            }
        }
    }
    Icon::from_rgba(rgba, 16, 16).unwrap()
}

// === Auto-start (registry) ===

const REG_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_NAME: &str = "VBANEmitter";

fn get_exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn is_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, KEY_READ, RegOpenKeyExW, RegQueryValueExW, RegCloseKey};
        use windows::core::w;
        unsafe {
            let mut hkey = Default::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"), Some(0), KEY_READ, &mut hkey).is_ok() {
                let mut buf = [0u16; 512];
                let mut size = (buf.len() * 2) as u32;
                let result = RegQueryValueExW(hkey, w!("VBANEmitter"), None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut size));
                let _ = RegCloseKey(hkey);
                return result.is_ok() && size > 0;
            }
        }
    }
    false
}

fn set_autostart(enable: bool) {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{
            HKEY_CURRENT_USER, KEY_WRITE, RegOpenKeyExW, RegSetValueExW, RegDeleteValueW, RegCloseKey,
        };
        use windows::core::w;
        unsafe {
            let mut hkey = Default::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"), Some(0), KEY_WRITE, &mut hkey).is_ok() {
                if enable {
                    let exe = get_exe_path();
                    let wide: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = RegSetValueExW(hkey, w!("VBANEmitter"), Some(0), 
                        windows::Win32::System::Registry::REG_SZ, 
                        { let data = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2); Some(data) },
                    );
                } else {
                    let _ = RegDeleteValueW(hkey, w!("VBANEmitter"));
                }
                let _ = RegCloseKey(hkey);
            }
        }
    }
}

// === Clipboard ===

fn copy_to_clipboard(text: &str) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{GlobalAlloc, GMEM_MOVEABLE};
        unsafe {
            if OpenClipboard(Some(HWND::default())).is_ok() {
                let _ = EmptyClipboard();
                let bytes = text
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect::<Vec<_>>();
                let size = bytes.len() * 2;
                let h = GlobalAlloc(GMEM_MOVEABLE, size);
                if let Ok(h) = h {
                    let ptr = windows::Win32::System::Memory::GlobalLock(h) as *mut u16;
                    if !ptr.is_null() {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                        let _ = windows::Win32::System::Memory::GlobalUnlock(h);
                        let _ = SetClipboardData(0x000D, Some(windows::Win32::Foundation::HANDLE(h.0)));
                    }
                }
                let _ = CloseClipboard();
            }
        }
    }
}

// === Status ===

#[derive(Clone, Debug)]
struct StatusInfo {
    device: String,
    audio_format: String,
    wasapi_buf: String,
    vban_frame: String,
    target: String,
    stream: String,
    pkt_count: u64,
    total_kb: u64,
    pkt_rate: f64,
    kbps: f64,
    ring_len: usize,
    drops: u64,
    elapsed: f64,
    error_msg: Option<String>,
    paused: bool,
}

impl StatusInfo {
    fn to_menu_text(&self) -> String {
        if self.paused {
            "⏸ Paused".to_string()
        } else {
            format!(
                "#{:.0}  {:.0}pkt/s  {:.0}KB/s  ring:{}  drop:{}",
                self.pkt_count, self.pkt_rate, self.kbps, self.ring_len, self.drops
            )
        }
    }

    fn to_clipboard(&self) -> String {
        let mut s = String::new();
        s.push_str("=== VBAN Emitter Status ===\n");
        s.push_str(&format!("Target:    {}\n", self.target));
        s.push_str(&format!("Stream:    {}\n", self.stream));
        s.push_str(&format!("Device:    {}\n", self.device));
        s.push_str(&format!("Audio:     {}\n", self.audio_format));
        s.push_str(&format!("WASAPI:    {}\n", self.wasapi_buf));
        s.push_str(&format!("VBAN:      {}\n", self.vban_frame));
        s.push_str(&format!("Packets:   {}\n", self.pkt_count));
        s.push_str(&format!("Data:      {} KB\n", self.total_kb));
        s.push_str(&format!("Rate:      {:.1} pkt/s\n", self.pkt_rate));
        s.push_str(&format!("Bandwidth: {:.0} KB/s\n", self.kbps));
        s.push_str(&format!("Ring buf:  {} bytes\n", self.ring_len));
        s.push_str(&format!("Drops:     {}\n", self.drops));
        s.push_str(&format!("Uptime:    {:.1}s\n", self.elapsed));
        s.push_str(&format!("Paused:    {}\n", self.paused));
        if let Some(ref e) = self.error_msg {
            s.push_str(&format!("Error:     {}\n", e));
        }
        s
    }
}

// === Audio thread ===

fn audio_loop(
    ip: String,
    port: u16,
    stream: String,
    bufsize: usize,
    ring_capacity: usize,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    status_tx: mpsc::Sender<StatusInfo>,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        audio_loop_inner(ip, port, stream, bufsize, ring_capacity, running, paused, status_tx);
    }));
    if let Err(e) = result {
        let msg = e.downcast_ref::<String>().cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "Unknown panic".to_string());
        eprintln!("Audio thread panic: {}", msg);
    }
}

fn audio_loop_inner(
    ip: String,
    port: u16,
    stream: String,
    bufsize: usize,
    ring_capacity: usize,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    status_tx: mpsc::Sender<StatusInfo>,
) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS};
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }

    initialize_mta().ok();
    let dev = match get_default_device(&Direction::Render) {
        Ok(d) => d,
        Err(e) => {
            let _ = status_tx.send(StatusInfo {
                device: format!("ERROR: {}", e), audio_format: String::new(),
                wasapi_buf: String::new(), vban_frame: String::new(),
                target: format!("{}:{}", ip, port), stream,
                pkt_count: 0, total_kb: 0, pkt_rate: 0.0, kbps: 0.0,
                ring_len: 0, drops: 0, elapsed: 0.0,
                error_msg: Some(format!("No render device: {}", e)), paused: false,
            });
            return;
        }
    };
    let dev_name = dev.get_friendlyname().unwrap_or_default();
    let mut client = dev.get_iaudioclient().expect("No AudioClient");

    let native_fmt = client.get_mixformat().unwrap();
    let sr = native_fmt.get_samplespersec() as usize;
    let ch = native_fmt.get_nchannels() as usize;

    let s16_fmt = WaveFormat::new(16, 16, &SampleType::Int, sr, ch, None);
    let init_ok = client
        .initialize_client(&s16_fmt, 0, &Direction::Capture, &ShareMode::Shared, true)
        .is_ok();

    if !init_ok {
        client = dev.get_iaudioclient().unwrap();
        client.initialize_client(&native_fmt, 0, &Direction::Capture, &ShareMode::Shared, false).unwrap();
    }

    let buffer_frames = client.get_bufferframecount().unwrap() as usize;
    let cap = client.get_audiocaptureclient().unwrap();
    let evt = client.set_get_eventhandle().unwrap();
    client.start_stream().unwrap();

    let src_bps = if init_ok { 2 } else { 4 };
    let src_bpf = ch * src_bps;
    let vban_bpf = ch * 2;
    let vban_frames = if bufsize > 0 { bufsize.min(256) } else { buffer_frames.min(256) };
    let chunk = vban_frames * vban_bpf;
    let sr_idx = get_sr_index(sr as u32);
    let is_float = !init_ok;

    let target = format!("{}:{}", ip, port);
    let sock = UdpSocket::bind("0.0.0.0:0").unwrap();

    let mut ring: VecDeque<u8> = VecDeque::with_capacity(ring_capacity);
    let mut fc: u32 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_drops: u64 = 0;
    let t0 = Instant::now();
    let mut last_status = t0;
    let mut convert_buf = Vec::with_capacity(8192);

    while running.load(Ordering::Relaxed) {
        // If paused, just sleep and send paused status
        if paused.load(Ordering::Relaxed) {
            let _ = status_tx.send(StatusInfo {
                device: format!("{} [Loopback]", dev_name),
                audio_format: format!("{}Hz / {}ch / {}bps ({})", sr, ch, src_bps * 8, if is_float { "float" } else { "int" }),
                wasapi_buf: format!("{} frames ({:.1}ms)", buffer_frames, buffer_frames as f64 / sr as f64 * 1000.0),
                vban_frame: format!("{} frames ({:.1}ms)", vban_frames, vban_frames as f64 / sr as f64 * 1000.0),
                target: target.clone(), stream: stream.clone(),
                pkt_count: fc as u64, total_kb: total_bytes / 1024,
                pkt_rate: 0.0, kbps: 0.0, ring_len: ring.len(), drops: total_drops,
                elapsed: t0.elapsed().as_secs_f64(), error_msg: None, paused: true,
            });
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }

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
                        convert_buf.clear();
                        convert_buf.reserve(len / 2);
                        for c in buf[..len].chunks_exact(4) {
                            let s = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                            let v = (s.max(-1.0).min(1.0) * 32767.0) as i16;
                            convert_buf.extend_from_slice(&v.to_le_bytes());
                        }
                        while ring.len() + convert_buf.len() > ring_capacity {
                            let drain = chunk.min(ring.len());
                            ring.drain(..drain);
                            total_drops += 1;
                        }
                        ring.extend(convert_buf.iter());
                    } else {
                        while ring.len() + len > ring_capacity {
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
            let pkt = build_packet(&stream, sr_idx, ch as u8, fc, nb, &pcm);
            let _ = sock.send_to(&pkt, &target);
            fc += 1;
            total_bytes += pcm.len() as u64;
        }

        let now = Instant::now();
        if now.duration_since(last_status).as_secs_f64() >= 2.0 {
            let elapsed = now.duration_since(t0).as_secs_f64();
            let _ = status_tx.send(StatusInfo {
                device: format!("{} [Loopback]", dev_name),
                audio_format: format!("{}Hz / {}ch / {}bps ({})", sr, ch, src_bps * 8, if is_float { "float" } else { "int" }),
                wasapi_buf: format!("{} frames ({:.1}ms)", buffer_frames, buffer_frames as f64 / sr as f64 * 1000.0),
                vban_frame: format!("{} frames ({:.1}ms)", vban_frames, vban_frames as f64 / sr as f64 * 1000.0),
                target: target.clone(), stream: stream.clone(),
                pkt_count: fc as u64, total_kb: total_bytes / 1024,
                pkt_rate: fc as f64 / elapsed, kbps: total_bytes as f64 / elapsed / 1024.0,
                ring_len: ring.len(), drops: total_drops, elapsed, error_msg: None, paused: false,
            });
            last_status = now;
        }
    }

    client.stop_stream().unwrap();
    deinitialize();
}

fn main() {
    let args = Args::parse();

    if args.list {
        list_devices();
        return;
    }

    let (status_tx, status_rx) = mpsc::channel::<StatusInfo>();
    let latest_status: Arc<Mutex<Option<StatusInfo>>> = Arc::new(Mutex::new(None));
    let latest_for_clip = latest_status.clone();

    if args.no_tray {
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let r = running.clone();
        ctrlc::set_handler(move || { r.store(false, Ordering::Relaxed); }).ok();
        audio_loop(args.ip, args.port, args.stream, args.bufsize, args.ring_capacity, running, paused, status_tx);
        while let Ok(s) = status_rx.try_recv() { println!("{}", s.to_clipboard()); }
        return;
    }

    // === Tray mode ===
    let running = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));

    let r = running.clone();
    let p = paused.clone();
    let _audio_thread = thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            audio_loop(args.ip, args.port, args.stream, args.bufsize, args.ring_capacity, r, p, status_tx);
        })
        .unwrap();

    // Menu
    let menu = Menu::new();
    let status_item = MenuItem::new("Starting...", false, None);
    let pause_item = MenuItem::new("Pause", true, None);
    let copy_item = MenuItem::new("Copy Info", true, None);
    let startup_item = MenuItem::new("Start with Windows", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    menu.append(&status_item).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&pause_item).unwrap();
    menu.append(&copy_item).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&startup_item).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&quit_item).unwrap();

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(false)
        .with_tooltip("VBAN Emitter")
        .with_icon(create_icon())
        .build()
        .unwrap();

    // Update startup menu text
    let update_startup_text = |item: &MenuItem| {
        if is_autostart_enabled() {
            let _ = item.set_text("☑ Start with Windows");
        } else {
            let _ = item.set_text("☐ Start with Windows");
        }
    };
    update_startup_text(&startup_item);

    // Event loop
    let event_loop = winit::event_loop::EventLoop::new().unwrap();
    event_loop
        .run(move |event, elwt| {
            // Poll status
            while let Ok(status) = status_rx.try_recv() {
                let _ = status_item.set_text(&status.to_menu_text());
                if let Ok(mut s) = latest_status.lock() {
                    *s = Some(status);
                }
            }

            // Menu events
            if let Ok(event) = muda::MenuEvent::receiver().try_recv() {
                if event.id == pause_item.id() {
                    let is_paused = paused.load(Ordering::Relaxed);
                    paused.store(!is_paused, Ordering::Relaxed);
                    if is_paused {
                        let _ = pause_item.set_text("Pause");
                    } else {
                        let _ = pause_item.set_text("▶ Resume");
                    }
                } else if event.id == copy_item.id() {
                    if let Ok(s) = latest_for_clip.lock() {
                        if let Some(ref info) = *s {
                            copy_to_clipboard(&info.to_clipboard());
                        }
                    }
                } else if event.id == startup_item.id() {
                    let enabled = is_autostart_enabled();
                    set_autostart(!enabled);
                    update_startup_text(&startup_item);
                } else if event.id == quit_item.id() {
                    running.store(false, Ordering::Relaxed);
                    elwt.exit();
                }
            }

            // Left click: copy to clipboard
            if let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    if let Ok(s) = latest_for_clip.lock() {
                        if let Some(ref info) = *s {
                            copy_to_clipboard(&info.to_clipboard());
                        }
                    }
                }
            }

            if let winit::event::Event::AboutToWait = event {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        })
        .unwrap();
}
