# rust-fb-remote

GPL-3.0-or-later Rust remote framebuffer viewer for Linux devices. The client connects through OpenSSH, displays RGB frames in a desktop window, and forwards basic mouse/keyboard events to the remote agent.

## Prototype limitations

This is an early MVP. It assumes tightly packed framebuffer rows and explicit width/height/bpp values; many real /dev/fb0 devices have padded scanlines or non-default channel bitfields. Input forwarding currently uses xdotool (X11), so it does not yet simulate input on a bare framebuffer or Wayland-only device. Do not expose it to untrusted networks.

## Build

Install Rust stable, OpenSSH client, and Linux GUI build dependencies required by eframe, then run:

    cargo build --release

## Remote device

Copy/install the same binary on the remote device. Ensure the SSH account can read /dev/fb0 and can use xdotool in an active X11 session if input forwarding is wanted. Prefer a dedicated account/group and least privilege rather than making the framebuffer world-readable.

Example, adjust values to match the device:

    cargo run --release -- agent --device /dev/fb0 --width 800 --height 600 --bpp 32

## Client

The client starts the remote command over SSH:

    cargo run --release -- connect user@device --agent /usr/local/bin/rust-fb-remote

The client uses OpenSSH normal host-key verification and authentication. Verify the remote host fingerprint before accepting it.

## Wire protocol

The agent writes repeated records to stdout: ASCII header FBREMOTE1 WIDTH HEIGHT followed by exactly WIDTH * HEIGHT * 3 RGB bytes. Input commands arrive on stdin as newline-delimited MOVE x y, CLICK button x y, KEY_DOWN key, and KEY_UP key. Standard error is reserved for diagnostics.

## License

Licensed under the GNU General Public License v3.0 or (at your option) any later version. Full license text: https://www.gnu.org/licenses/gpl-3.0.txt
