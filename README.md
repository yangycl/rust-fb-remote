# rust-fb-remote

GPL-3.0-or-later Rust remote framebuffer viewer for Linux devices. The Windows/Linux desktop client can launch the remote agent over OpenSSH, or attach directly to an agent started manually over TCP.

## What was fixed (v0.1.1)

The earlier MVP produced a black screen on most real devices because it:

- Assumed tightly-packed rows (no stride / line_length padding)
- Hard-coded 800×600 / 32 bpp
- Used a fixed BGR conversion instead of the device's actual bitfields

The agent now:

- Queries the real resolution, bpp and **line_length** via `FBIOGET_VSCREENINFO` / `FBIOGET_FSCREENINFO`
- Honours the reported red/green/blue bitfields
- Falls back to common BGR layouts only when bitfields are missing
- Defaults to auto-detection (pass `--width 0 --height 0 --bpp 0` or omit them)

Input forwarding still uses xdotool (X11). Do not expose the TCP listener to untrusted networks.

## Build

Install Rust stable, OpenSSH client, and Linux GUI build dependencies required by eframe, then run:

    cargo build --release

## Remote device: manually started TCP agent

The remote device needs the same binary. Start the agent with permission to read `/dev/fb0`. Bind it to the remote device's Tailscale IP (recommended) and choose a TCP port:

    # Auto-detect everything (recommended)
    sudo ./target/release/rust-fb-remote agent --listen 100.78.63.74:24800

    # Or override if needed
    sudo ./target/release/rust-fb-remote agent --listen 100.78.63.74:24800 --device /dev/fb0 --width 1920 --height 1080 --bpp 32

Replace `100.78.63.74` with the Linux device's actual Tailscale IP. Keep this terminal open while using the viewer. The agent accepts one viewer at a time and waits for another connection after the viewer disconnects.

If UFW is enabled, permit the port only on the Tailscale interface:

    sudo ufw allow in on tailscale0 to any port 24800 proto tcp

Do not bind to `0.0.0.0` or expose port 24800 to the public internet. This TCP mode does not provide encryption or authentication by itself; use it only on a trusted private network such as a properly configured Tailscale network.

## Client mode A: attach to the manually started agent

On Windows PowerShell, run:

    .\rust-fb-remote.exe attach 100.78.63.74:24800

On Linux, run:

    cargo run --release -- attach 100.78.63.74:24800

The viewer connects to the agent that is already running; it does not invoke SSH or sudo.

## Client mode B: launch the agent over SSH

The client can still start the remote command over SSH:

    cargo run --release -- connect user@device --agent /usr/local/bin/rust-fb-remote

The client uses OpenSSH normal host-key verification and authentication. Verify the remote host fingerprint before accepting it. The SSH account must have permission to read `/dev/fb0`, or the remote launch command must be configured separately to obtain the required privileges.

The remote agent now auto-detects the real framebuffer geometry; no more hard-coded 800×600.

## Wire protocol

The agent writes repeated records to the connection: ASCII header `FBREMOTE1 WIDTH HEIGHT` followed by exactly `WIDTH * HEIGHT * 3` RGB bytes. Input commands arrive on the same connection as newline-delimited `MOVE x y`, `CLICK button x y`, `KEY_DOWN key`, and `KEY_UP key`. Standard error is reserved for diagnostics.

## License

Licensed under the GNU General Public License v3.0 or (at your option) any later version. Full license text: https://www.gnu.org/licenses/gpl-3.0.txt
