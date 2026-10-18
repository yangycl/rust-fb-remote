use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use eframe::egui;
use std::{io::{Read, Write}, process::{ChildStdout, Command, Stdio}, sync::mpsc, thread, time::Duration};

#[derive(Parser)]
#[command(name="rust-fb-remote", version, about="GPL SSH framebuffer remote desktop")]
struct Cli { #[command(subcommand)] command: Mode }
#[derive(Subcommand)]
enum Mode {
    Connect { host: String, #[arg(long, default_value="rust-fb-remote-agent")] agent: String },
    Agent { #[arg(long, default_value="/dev/fb0")] device: String, #[arg(long, default_value_t=800)] width: u32, #[arg(long, default_value_t=600)] height: u32, #[arg(long, default_value_t=32)] bpp: u32 },
}
struct Frame { width: usize, height: usize, rgb: Vec<u8> }
fn main() -> Result<()> {
    match Cli::parse().command {
        Mode::Agent { device, width, height, bpp } => agent::run(&device, width, height, bpp),
        Mode::Connect { host, agent } => connect(host, agent),
    }
}
fn connect(host: String, agent: String) -> Result<()> {
    // OpenSSH preserves normal known_hosts checking and authentication.
    let mut child = Command::new("ssh").args(["-T", &host, &agent, "agent", "--width", "800", "--height", "600"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn()
        .context("failed to launch ssh; install OpenSSH client")?;
    let input = child.stdin.take().context("ssh stdin unavailable")?;
    let output = child.stdout.take().context("ssh stdout unavailable")?;
    let (tx, rx) = mpsc::channel::<Frame>();
    thread::spawn(move || { if let Err(e) = read_frames(output, tx) { eprintln!("frame stream ended: {e:#}"); } });
    let app = Viewer { rx, texture: None, dimensions: [800,600], input, status: "Connected; waiting for framebuffer".into() };
    let result = eframe::run_native("rust-fb-remote", eframe::NativeOptions::default(), Box::new(|_| Ok(Box::new(app))));
    let _ = child.kill();
    result.map_err(|e| anyhow::anyhow!("GUI failed: {e}"))
}
fn read_frames(mut stream: ChildStdout, tx: mpsc::Sender<Frame>) -> Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear(); let mut byte = [0u8;1];
        loop { stream.read_exact(&mut byte)?; if byte[0] == b'\n' { break; } line.push(byte[0]); if line.len() > 256 { bail!("invalid frame header"); } }
        let header = std::str::from_utf8(&line)?;
        let mut p = header.split_whitespace();
        if p.next() != Some("FBREMOTE1") { bail!("unexpected protocol header: {header}"); }
        let w: usize = p.next().context("missing width")?.parse()?;
        let h: usize = p.next().context("missing height")?.parse()?;
        if w == 0 || h == 0 || w.checked_mul(h).and_then(|n| n.checked_mul(3)).unwrap_or(usize::MAX) > 100_000_000 { bail!("unsafe frame dimensions"); }
        let mut rgb = vec![0; w*h*3]; stream.read_exact(&mut rgb)?;
        tx.send(Frame { width:w, height:h, rgb }).map_err(|_| anyhow::anyhow!("viewer closed"))?;
    }
}
struct Viewer { rx: mpsc::Receiver<Frame>, texture: Option<egui::TextureHandle>, dimensions: [usize;2], input: std::process::ChildStdin, status: String }
impl Viewer { fn send(&mut self, event: &str) { if writeln!(self.input, "{event}").and_then(|_| self.input.flush()).is_err() { self.status = "Input channel closed".into(); } } }
impl eframe::App for Viewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(frame) = self.rx.try_recv() {
            self.dimensions = [frame.width, frame.height];
            let image = egui::ColorImage::from_rgb(self.dimensions, &frame.rgb);
            if let Some(t) = &mut self.texture { t.set(image, egui::TextureOptions::NEAREST); }
            else { self.texture = Some(ctx.load_texture("remote-framebuffer", image, egui::TextureOptions::NEAREST)); }
        }
        egui::TopBottomPanel::top("status").show(ctx, |ui| { ui.label(format!("{} · {}×{} · GPL-3.0-or-later", self.status, self.dimensions[0], self.dimensions[1])); });
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(tex) = &self.texture {
                let avail = ui.available_size();
                let scale = (avail.x / self.dimensions[0] as f32).min(avail.y / self.dimensions[1] as f32).max(0.01);
                let size = egui::vec2(self.dimensions[0] as f32*scale, self.dimensions[1] as f32*scale);
                let response = ui.add(egui::Image::new(tex).fit_to_exact_size(size).sense(egui::Sense::click_and_drag()));
                if response.hovered() {
                    if let Some(pos) = response.hover_pos() {
                        let x = ((pos.x-response.rect.min.x)/response.rect.width()*self.dimensions[0] as f32) as i32;
                        let y = ((pos.y-response.rect.min.y)/response.rect.height()*self.dimensions[1] as f32) as i32;
                        if response.dragged() { self.send(&format!("MOVE {x} {y}")); }
                        if response.clicked() { self.send(&format!("CLICK 1 {x} {y}")); }
                    }
                }
            } else { ui.spinner(); ui.label("Waiting for remote agent…"); }
        });
        let events = ctx.input(|i| i.events.clone());
        for event in events { if let egui::Event::Key { key, pressed, repeat:false, .. } = event { self.send(&format!("KEY_{} {:?}", if pressed {"DOWN"} else {"UP"}, key)); } }
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
mod agent;
