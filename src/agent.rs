use anyhow::{bail, Context, Result};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

pub fn run(device: &str, width: u32, height: u32, bpp: u32) -> Result<()> {
    run_stream(
        device,
        width,
        height,
        bpp,
        std::io::stdin(),
        std::io::stdout(),
    )
}

/// Listen for one viewer at a time. Bind to a private interface (such as Tailscale),
/// not 0.0.0.0, unless access is deliberately protected by a firewall.
pub fn run_listener(
    address: &str,
    device: &str,
    width: u32,
    height: u32,
    bpp: u32,
) -> Result<()> {
    let listener = TcpListener::bind(address)
        .with_context(|| format!("cannot listen on {address}; check address and port"))?;
    eprintln!("rust-fb-remote agent listening on {address}");
    loop {
        let (stream, peer) = listener.accept().context("failed to accept viewer connection")?;
        eprintln!("viewer connected from {peer}");
        if let Err(error) = serve_client(stream, device, width, height, bpp) {
            eprintln!("viewer disconnected: {error:#}");
        }
    }
}

fn serve_client(
    stream: TcpStream,
    device: &str,
    width: u32,
    height: u32,
    bpp: u32,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let input = stream.try_clone().context("cannot clone client TCP stream")?;
    run_stream(device, width, height, bpp, input, stream)
}

fn run_stream<R, W>(
    device: &str,
    width: u32,
    height: u32,
    bpp: u32,
    input: R,
    mut out: W,
) -> Result<()>
where
    R: Read + Send + 'static,
    W: Write,
{
    if width == 0 || height == 0 || width as u64 * height as u64 > 20_000_000 {
        bail!("invalid framebuffer dimensions");
    }
    if ![16, 24, 32].contains(&bpp) {
        bail!("supported bpp values: 16, 24, 32");
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_input = stop.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(input);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let p: Vec<_> = line.split_whitespace().collect();
            match p.as_slice() {
                ["MOVE", x, y] => {
                    let _ = Command::new("xdotool").args(["mousemove", x, y]).status();
                }
                ["CLICK", button, x, y] => {
                    let _ = Command::new("xdotool")
                        .args(["mousemove", x, y, "click", button])
                        .status();
                }
                ["KEY_DOWN", key] => {
                    let _ = Command::new("xdotool").args(["keydown", key]).status();
                }
                ["KEY_UP", key] => {
                    let _ = Command::new("xdotool").args(["keyup", key]).status();
                }
                ["QUIT"] => {
                    stop_input.store(true, Ordering::Relaxed);
                    break;
                }
                _ => eprintln!("ignored malformed input event"),
            }
        }
        stop_input.store(true, Ordering::Relaxed);
    });

    let mut fb =
        File::open(device).with_context(|| format!("cannot open {device}; check device permissions"))?;
    let pixels = width as usize * height as usize;
    let bytes_per_pixel = (bpp / 8) as usize;
    let mut raw = vec![0u8; pixels * bytes_per_pixel];
    let mut rgb = vec![0u8; pixels * 3];
    eprintln!("rust-fb-remote agent: {device}, {width}x{height}, {bpp} bpp");

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // fbdev is a finite memory mapping exposed as a file, not an endless video stream.
        // Rewind before sampling each frame; otherwise the file offset reaches EOF.
        fb.seek(SeekFrom::Start(0))
            .context("cannot seek framebuffer to the start")?;
        fb.read_exact(&mut raw).with_context(|| {
            format!(
                "could not read {} bytes from {device}; check width/height/bpp and framebuffer line padding",
                raw.len()
            )
        })?;

        for i in 0..pixels {
            let s = i * bytes_per_pixel;
            let d = i * 3;
            match bpp {
                16 => {
                    let v = u16::from_ne_bytes([raw[s], raw[s + 1]]);
                    let r = ((v >> 11) & 31) as u8;
                    let g = ((v >> 5) & 63) as u8;
                    let b = (v & 31) as u8;
                    rgb[d] = r * 255 / 31;
                    rgb[d + 1] = g * 255 / 63;
                    rgb[d + 2] = b * 255 / 31;
                }
                24 | 32 => {
                    rgb[d] = raw[s + 2];
                    rgb[d + 1] = raw[s + 1];
                    rgb[d + 2] = raw[s];
                }
                _ => unreachable!(),
            }
        }
        write!(out, "FBREMOTE1 {width} {height}\n")?;
        out.write_all(&rgb)?;
        out.flush()?;
        thread::sleep(Duration::from_millis(33));
    }
    Ok(())
}
