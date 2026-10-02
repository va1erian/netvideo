# netvideo

A small, security-first home-lab video server: a simpler alternative to
Jellyfin that only does video and shows your library exactly as your folders
lay it out. Clients pair with a one-time code and stream from anywhere.

- **Server**: Rust, Linux, Docker, behind Cosmos Cloud (`crates/server`).
- **Clients**: Android (Kotlin) and an XUI desktop app for Windows and macOS
  (planned).

See [docs/PLAN.md](docs/PLAN.md) for the design and milestones,
[docs/security.md](docs/security.md) for the security model and
[AGENTS.md](AGENTS.md) for contributor rules.

## Running the server

```bash
export NETVIDEO_LIBRARY_PATHS=/path/to/videos
export NETVIDEO_DATA_DIR=./data
export NETVIDEO_FFPROBE=ffprobe              # optional; videos are indexed unprobed without it
cargo run -p netvideo-server -- serve
cargo run -p netvideo-server -- pair      # one-time code for an admin device
cargo run -p netvideo-server -- pair --qr --url https://video.example  # plus a QR code for the app
cargo run -p netvideo-server -- devices   # list paired devices
```

For Docker and Cosmos Cloud, see [deploy/docker-compose.yml](deploy/docker-compose.yml).
