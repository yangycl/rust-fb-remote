use anyhow::{bail, Context, Result};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    os::fd::AsRawFd,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

// Linux framebuffer ioctl constants and structs (from linux/fb.h)
const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

impl Default for FbVarScreeninfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

impl Default for FbFixScreeninfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[derive(Debug, Clone)]
struct FbInfo {
    width: u32,
    height: u32,
    bpp: u32,
    line_length: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
}

fn query_fb_info(device: &str) -> Result<FbInfo> {
    let file = File::open(device)
        .with_context(|| format!("cannot open {device}; need read permission (root or video group)"))?;
    let fd = file.as_raw_fd();

    let mut var = FbVarScreeninfo::default();
    let mut fix = FbFixScreeninfo::default();

    let ret = unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var) };
    if ret != 0 {
        bail!(
            "FBIOGET_VSCREENINFO failed on {device}: {}",
            std::io::Error::last_os_error()
        );
    }
    let ret = unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix) };
    if ret != 0 {
        bail!(
            "FBIOGET_FSCREENINFO failed on {device}: {}",
            std::io::Error::last_os_error()
        );
    }

    if var.xres == 0 || var.yres == 0 {
        bail!("framebuffer reports zero resolution");
    }
    if var.bits_per_pixel == 0 || var.bits_per_pixel > 32 {
        bail!("unsupported bits_per_pixel: {}", var.bits_per_pixel);
    }
    if fix.line_length == 0 {
        // Fallback: tightly packed
        fix.line_length = var.xres * ((var.bits_per_pixel + 7) / 8);
    }

    eprintln!(
        "fb info: {}x{} {}bpp, line_length={}, red={:?} green={:?} blue={:?}",
        var.xres, var.yres, var.bits_per_pixel, fix.line_length, var.red, var.green, var.blue
    );

    Ok(FbInfo {
        width: var.xres,
        height: var.yres,
        bpp: var.bits_per_pixel,
        line_length: fix.line_length,
        red: var.red,
        green: var.green,
        blue: var.blue,
    })
}

/// Extract a color component from a pixel value using the bitfield description.
fn extract_component(pixel: u32, bf: &FbBitfield) -> u8 {
    if bf.length == 0 {
        return 0;
    }
    let mut val = (pixel >> bf.offset) & ((1u32 << bf.length) - 1);
    if bf.msb_right != 0 {
        // rare; reverse bits if needed – most drivers leave msb_right=0
        val = val.reverse_bits() >> (32 - bf.length);
    }
    // Scale to 8-bit
    if bf.length >= 8 {
        (val >> (bf.length - 8)) as u8
    } else {
        ((val * 255) / ((1u32 << bf.length) - 1)) as u8
    }
}

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
        let (stream, peer) = listener
            .accept()
            .context("failed to accept viewer connection")?;
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
    let input = stream
        .try_clone()
        .context("cannot clone client TCP stream")?;
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
    // Always query real device parameters. User-supplied width/height/bpp override when non-zero.
    let mut info = query_fb_info(device)?;
    if width != 0 {
        info.width = width;
    }
    if height != 0 {
        info.height = height;
    }
    if bpp != 0 {
        info.bpp = bpp;
    }

    if info.width == 0
        || info.height == 0
        || (info.width as u64) * (info.height as u64) > 20_000_000
    {
        bail!(
            "invalid framebuffer dimensions {}x{}",
            info.width,
            info.height
        );
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

    let mut fb = File::open(device)
        .with_context(|| format!("cannot open {device}; check device permissions"))?;

    let bytes_per_pixel = ((info.bpp + 7) / 8) as usize;
    let row_bytes = info.line_length as usize;
    let expected_min = (info.width as usize) * bytes_per_pixel;
    if row_bytes < expected_min {
        bail!(
            "line_length {} is smaller than width*bpp/8 ({}); framebuffer info looks broken",
            row_bytes,
            expected_min
        );
    }

    // One full frame buffer (with stride)
    let mut raw = vec![0u8; row_bytes * info.height as usize];
    let mut rgb = vec![0u8; (info.width * info.height * 3) as usize];

    eprintln!(
        "rust-fb-remote agent: {device}, {}x{}, {} bpp, stride={} bytes/row",
        info.width, info.height, info.bpp, info.line_length
    );

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

        // Convert using real bitfields + stride
        for y in 0..info.height as usize {
            let row_start = y * row_bytes;
            for x in 0..info.width as usize {
                let s = row_start + x * bytes_per_pixel;
                let d = (y * info.width as usize + x) * 3;

                let pixel = match info.bpp {
                    16 => {
                        let v = u16::from_ne_bytes([raw[s], raw[s + 1]]);
                        v as u32
                    }
                    24 => {
                        // little-endian packed
                        u32::from_le_bytes([raw[s], raw[s + 1], raw[s + 2], 0])
                    }
                    32 => u32::from_ne_bytes([raw[s], raw[s + 1], raw[s + 2], raw[s + 3]]),
                    _ => {
                        // generic: take as many bytes as available
                        let mut v = 0u32;
                        for i in 0..bytes_per_pixel.min(4) {
                            v |= (raw[s + i] as u32) << (8 * i);
                        }
                        v
                    }
                };

                // Prefer the reported bitfields; fall back to common BGR/RGB layouts
                if info.red.length > 0 && info.green.length > 0 && info.blue.length > 0 {
                    rgb[d] = extract_component(pixel, &info.red);
                    rgb[d + 1] = extract_component(pixel, &info.green);
                    rgb[d + 2] = extract_component(pixel, &info.blue);
                } else {
                    // Legacy fallback (common on many embedded boards)
                    match info.bpp {
                        16 => {
                            let r = ((pixel >> 11) & 31) as u8;
                            let g = ((pixel >> 5) & 63) as u8;
                            let b = (pixel & 31) as u8;
                            rgb[d] = r * 255 / 31;
                            rgb[d + 1] = g * 255 / 63;
                            rgb[d + 2] = b * 255 / 31;
                        }
                        24 | 32 => {
                            // Assume BGRA / BGR (most common on Linux fb)
                            rgb[d] = ((pixel >> 16) & 0xff) as u8; // R
                            rgb[d + 1] = ((pixel >> 8) & 0xff) as u8; // G
                            rgb[d + 2] = (pixel & 0xff) as u8; // B
                        }
                        _ => {
                            rgb[d] = 0;
                            rgb[d + 1] = 0;
                            rgb[d + 2] = 0;
                        }
                    }
                }
            }
        }

        write!(out, "FBREMOTE1 {} {}\n", info.width, info.height)?;
        out.write_all(&rgb)?;
        out.flush()?;
        thread::sleep(Duration::from_millis(33));
    }
    Ok(())
}
