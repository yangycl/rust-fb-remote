use anyhow::{bail, Context, Result};
use std::{fs::File, io::{Read, Write, Seek, SeekFrom, BufRead, BufReader}, process::Command, sync::{Arc, atomic::{AtomicBool, Ordering}}, thread, time::Duration};

pub fn run(device: &str, width: u32, height: u32, bpp: u32) -> Result<()> {
    if width == 0 || height == 0 || width as u64 * height as u64 > 20_000_000 { bail!("invalid framebuffer dimensions"); }
    if ![16,24,32].contains(&bpp) { bail!("supported bpp values: 16, 24, 32"); }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_input = stop.clone();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in BufReader::new(stdin.lock()).lines().flatten() {
            let p: Vec<_> = line.split_whitespace().collect();
            match p.as_slice() {
                ["MOVE", x, y] => { let _ = Command::new("xdotool").args(["mousemove", x, y]).status(); }
                ["CLICK", button, x, y] => { let _ = Command::new("xdotool").args(["mousemove", x, y, "click", button]).status(); }
                ["KEY_DOWN", key] => { let _ = Command::new("xdotool").args(["keydown", key]).status(); }
                ["KEY_UP", key] => { let _ = Command::new("xdotool").args(["keyup", key]).status(); }
                ["QUIT"] => { stop_input.store(true, Ordering::Relaxed); break; }
                _ => eprintln!("ignored malformed input event"),
            }
        }
    });
    let mut fb = File::open(device).with_context(|| format!("cannot open {device}; check device permissions"))?;
    let pixels = width as usize * height as usize;
    let bytes_per_pixel = (bpp/8) as usize;
    let mut raw = vec![0u8; pixels*bytes_per_pixel];
    let mut rgb = vec![0u8; pixels*3];
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    eprintln!("rust-fb-remote agent: {device}, {width}x{height}, {bpp} bpp");
    loop {
        if stop.load(Ordering::Relaxed) { break; }
        // fbdev is a finite memory mapping exposed as a file, not an endless video stream.
        // Rewind to byte 0 before sampling each frame; otherwise the file offset reaches EOF.
        fb.seek(SeekFrom::Start(0)).context("cannot seek framebuffer to the start")?;
        fb.read_exact(&mut raw).with_context(|| format!("could not read {} bytes from {device}; check width/height/bpp and framebuffer line padding", raw.len()))?;
        for i in 0..pixels {
            let s = i*bytes_per_pixel; let d=i*3;
            match bpp {
                16 => { let v=u16::from_ne_bytes([raw[s],raw[s+1]]); let r=((v>>11)&31) as u8; let g=((v>>5)&63) as u8; let b=(v&31) as u8; rgb[d]=r*255/31; rgb[d+1]=g*255/63; rgb[d+2]=b*255/31; }
                24 => { rgb[d]=raw[s+2]; rgb[d+1]=raw[s+1]; rgb[d+2]=raw[s]; }
                32 => { rgb[d]=raw[s+2]; rgb[d+1]=raw[s+1]; rgb[d+2]=raw[s]; }
                _ => unreachable!()
            }
        }
        write!(out, "FBREMOTE1 {width} {height}\n")?; out.write_all(&rgb)?; out.flush()?;
        thread::sleep(Duration::from_millis(33));
    }
    Ok(())
}
