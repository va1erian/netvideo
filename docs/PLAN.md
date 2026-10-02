# netvideo — plan

netvideo is a home-lab video streaming server: a much simpler alternative to
Jellyfin that only does video. It indexes folders of video files, lets paired
devices browse them as they are laid out on disk, and streams each video in a
form the client can play, transcoding only when it has to.

This document is the starting plan. It records the decisions taken so far,
what is reused from [emusic](https://github.com/va1erian/emusic) and
[xui](https://github.com/va1erian/xui), the milestones, and the questions
still open.

## 1. Goals and non-goals

**Goals**

- Scan one or more library folders into a database of videos.
- Present the library exactly as the filesystem lays it out: roots, folders,
  files. No grouping into shows, seasons or movies.
- Pair clients with a short, strongly secured pairing step (the emusic-server
  mechanism), then authenticate every request.
- Stream each video in the best form for the client: the original file when
  the client can play it, a remux when only the container is wrong, a
  transcode when the codecs are wrong or the bandwidth is too low.
- Server in Rust, running on Linux in Docker behind Cosmos Cloud.
- Clients: a Kotlin Android app based on the emusic Android client, and an
  XUI desktop app for Windows and macOS.
- Security is a first-class requirement, not a later pass.

**Non-goals (for now)**

- Metadata scraping (TMDB/TVDB), posters, show/season grouping, collections.
- User accounts with passwords, multi-user profiles, parental controls.
- Live TV, DVR, music, photos, plugins, a web UI.
- Casting (Chromecast/DLNA), offline downloads, sync-play.
- Linux desktop client (XUI runs there, but it is not a target yet).

## 2. Architecture

```text
 Android app (Kotlin, Compose, Media3) ─┐
                                        │  HTTPS (TLS at Cosmos)
 Desktop app (XUI, Windows/macOS) ──────┤
                                        ▼
                         ┌────────── Cosmos Cloud ──────────┐
                         │ reverse proxy, Let's Encrypt TLS │
                         └───────────────┬──────────────────┘
                                         │ plain HTTP on the Docker network
                         ┌───────────────▼──────────────────┐
                         │ netvideo-server (Rust, axum)     │
                         │  auth · library API · streaming  │
                         │  scanner · transcode manager     │
                         └───┬───────────┬──────────────┬───┘
                             │           │              │
                      SQLite (data) media (read-only)  ffmpeg/ffprobe
                                                       (child processes,
                                                        transcode cache)
```

Both clients share one Rust client core (pairing, token refresh, credential
storage, API calls), exposed to Kotlin through UniFFI exactly as emusic's
`crates/mobile` does today, and used directly by the desktop app.

### Repository layout

One Cargo workspace (edition 2024, `members = ["crates/*"]`), plus the
Android project, mirroring emusic:

| Path | What it is |
|---|---|
| `crates/server` | `netvideo-server` binary + lib: config, auth, API, scanner, DB, transcoding. |
| `crates/proto` | Shared API types (serde structs for requests and responses, capability profiles). No I/O. |
| `crates/client` | Blocking client core (ureq + pasetors): pairing, refresh proofs, credential store, API calls. |
| `crates/mobile` | UniFFI cdylib over `client` for Android. |
| `crates/desktop` | The XUI desktop app (Windows, macOS). |
| `crates/xui-video` | The video widget for XUI (see §7). May move upstream into xui once stable. |
| `android/` | Kotlin app, Gradle + cargo-ndk + UniFFI, as in emusic. |
| `deploy/` | Dockerfile, docker-compose for Cosmos, example config. |
| `docs/` | This plan, the API reference, the security model, operations. |

## 3. Reuse from emusic

The emusic-server crate is mostly standalone (its only workspace dependency is
the SID renderer), so the plan is to **copy and adapt** the generic modules
into netvideo rather than share a crate across repos for now (decided, §11).

| emusic source | Use in netvideo |
|---|---|
| `crates/server/src/auth/` (keys, paseto, pairing, middleware) | Copied. Domain strings change from `emusic-server/...` to `netvideo/...`, scopes become `library:read stream:read admin`. |
| `crates/server/src/security/` (path_jail, rate_limit, proxy) | Copied as is, with its tests. `path_jail` guards every file the server opens. |
| `audit.rs`, `api/error.rs`, `api/range.rs`, `ws.rs`, `auth_routes.rs` | Copied. |
| `config.rs` (TOML + `EMUSIC_SERVER_*` env overrides) | Copied with a `NETVIDEO_*` prefix and video sections. |
| `db/mod.rs` (r2d2 + rusqlite, WAL, `user_version` migrations), `db/devices.rs` | Copied; new video schema. |
| `scanner/scan.rs` (`ScanCoordinator`, periodic + on-demand, WebSocket progress), `walk.rs`, tombstones, delta sync | Copied; tag reading replaced by ffprobe. |
| `crates/client` (auth, credentials, HTTP core) | Copied into `crates/client`. |
| `crates/mobile` UniFFI pattern | Copied, minus the renderer. |
| Android: Gradle/cargo-ndk/UniFFI wiring, server list + pairing screen, two-pane shell, authenticated `DefaultHttpDataSource` factory, theme | Copied into `android/`. |
| `deploy/Dockerfile`, `deploy/docker-compose.yml` hardening | Copied, plus ffmpeg and a transcode cache. |

Music-specific parts are left behind: lofty tags, SID/module/HVSC, the render
pipeline, album art, the track schema, album/artist/genre navigation, Android
Auto.

## 4. Server

### 4.1 Stack

axum 0.8, tokio, tower-http, rusqlite (bundled) + r2d2, pasetors, tracing,
clap, thiserror. `#![forbid(unsafe_code)]` across the crate. The server never
links libav; it drives `ffprobe` and `ffmpeg` as child processes (§6), which
keeps unsafe C out of the server's address space.

CLI, as in emusic: `netvideo-server serve` (default), `pair --ttl`, `devices`,
`revoke <id>`, plus `scan` to trigger a rescan.

### 4.2 Library and scanning

- Config lists library roots (`NETVIDEO_LIBRARY_PATHS=/media/videos,...`),
  each mounted read-only.
- A walk (walkdir, no symlink following outside the root) records every
  directory and every file with a known video extension (mkv, mp4, m4v, mov,
  avi, webm, ts, m2ts, wmv, mpg, ...). Hidden files and dot-directories are
  skipped.
- Incremental, like emusic: files whose size and mtime are unchanged are not
  re-probed. New or changed files are probed with `ffprobe -print_format json
  -show_format -show_streams` and the result is parsed into typed fields.
- Runs at startup, every `scan_interval_secs`, and on demand. A filesystem
  watcher (`notify`) is a later improvement; polling is enough to start.
- Deletions become tombstones so clients can delta-sync.
- Sidecar subtitles (`movie.srt`, `movie.en.srt`, `.ass`, `.vtt` next to the
  video) are recorded as external subtitle tracks.

Schema (first cut):

```text
roots       (id, path, name)
folders     (id, root_id, parent_id, rel_path, name, mtime, sync_version)
videos      (id, folder_id, file_name, size, mtime, probed_at,
             container, duration_ms, bitrate, sync_version)
streams     (video_id, index, kind[video|audio|subtitle], codec, profile,
             width, height, fps, hdr, channels, language, title, default, forced)
sidecars    (video_id, path, kind, language, format)
tombstones  (kind, id, sync_version, deleted_at)
devices, pairing_codes, meta  (from emusic)
progress    (device_id, video_id, position_ms, watched, updated_at)
```

IDs are opaque random 128-bit values (not autoincrement), so they reveal
nothing about library size or order. Paths never leave the server except as
display names relative to their root.

### 4.3 API (`/api/v1`)

| Method & path | Purpose |
|---|---|
| `GET health` | Liveness, no auth. |
| `POST auth/pair`, `POST auth/refresh` | From emusic. |
| `GET devices`, `POST devices/pairing-codes`, `DELETE devices/{id}` | From emusic, admin scope only (see §5). |
| `GET roots` | Library roots. |
| `GET folders/{id}?cursor=&limit=` | One folder's subfolders and videos, paginated, sorted by name (natural sort). |
| `GET videos/{id}` | Full metadata: streams, sidecars, duration. |
| `GET videos/{id}/file` | Direct play: the original bytes, single `Range` support, `ETag`. |
| `PUT videos/{id}/progress` | Body: `position_ms`, optional `watched`. Saves this device's resume position; folder pages and video details return it as `progress`. |
| `POST videos/{id}/playback` | Body: client capability profile, preferred audio/subtitle track, start position, max bitrate. Returns a playback plan (§6). |
| `GET sessions/{sid}/master.m3u8`, `.../{rendition}/index.m3u8`, `.../{seg}.m4s` | HLS for remux/transcode sessions. |
| `DELETE sessions/{sid}` | Client stops; server kills ffmpeg and frees the cache. |
| `GET videos/{id}/subtitles/{n}.vtt` | Subtitle extracted/converted to WebVTT. |
| `GET library/sync?since_version=` | Delta sync, as in emusic, for clients that cache the tree. |
| `GET ws` | Scan progress and library-changed events. |

Pagination is server-side from the start: a video library folder can hold
thousands of entries, and the emusic approach of shipping the whole library to
the phone does not scale here.

## 5. Security

The threat model: the server is reachable from the internet through Cosmos.
Anyone can reach the API; only paired devices may use it. The media files are
the owner's, but filenames and file contents are still treated as untrusted
input to parsers.

Kept from emusic:

- Pairing with a 6-digit, single-use, short-lived code; only an HMAC of the
  code is stored, keyed by the server secret that lives outside the DB.
  Identical 401s for wrong, used and expired codes. Per-IP rate limiting.
- Devices generate their own Ed25519 key; the server issues PASETO v4.public
  access tokens; refresh requires a proof signed by the device key, so a
  stolen token cannot be extended. Per-device revocation checked on every
  request.
- `X-Forwarded-For` only trusted from configured proxy networks (the Cosmos
  bridge).
- Path jail on every file access; violations look like 404.
- JSON-lines audit log of pairing, refresh, revocation and failures.
- Hardened container: non-root uid, read-only root FS, `cap_drop: ALL`,
  `no-new-privileges`, media mounted read-only.

Improved over emusic:

- **Roles.** Today any paired emusic device can mint pairing codes and revoke
  others. netvideo adds an `admin` flag per device: devices paired from a
  CLI code are admins unless it was minted with `--viewer`; codes minted by a device produce viewer devices
  unless the admin asks otherwise. Device management needs `admin`.
- **Pairing that resists a hostile network.** The pairing code can be shown
  as a QR code (`netvideo-server pair --qr`) that also carries the server URL
  and the fingerprint of the server's public key. The client checks that the
  token it receives is signed by that key, so a MITM during pairing fails even
  if TLS were compromised. Typing the 6-digit code by hand stays supported.
- **Client credential storage.** Android keeps the device private key wrapped
  by an Android Keystore key (non-exportable, hardware-backed where
  available) instead of a plain 0600 file. Windows uses DPAPI, macOS the
  Keychain.
- **No cleartext.** The Android manifest drops `usesCleartextTraffic`; a
  network security config allows cleartext only for explicit LAN-debug
  builds.
- **ffmpeg containment** (§6.4).
- **No secrets in URLs.** Media3 and the desktop player both send the
  `Authorization` header on every segment request, so HLS URLs carry no
  tokens. Session ids are random 128-bit and bound to the device that
  created them.
- **Supply chain.** `cargo deny` (advisories, licenses, sources) and
  `cargo audit` in CI; pinned image digests; Dependabot.

[security.md](security.md) holds the full model as built.

## 6. Playback and transcoding

### 6.1 Decision

The client sends a capability profile with every playback request:

```jsonc
{
  "containers": ["mp4", "mkv", "webm", "hls-fmp4"],
  "video": [{ "codec": "h264", "max_level": "5.1", "max_height": 2160 },
            { "codec": "hevc", "profiles": ["main", "main10"] },
            { "codec": "av1" }],
  "audio": [{ "codec": "aac", "max_channels": 6 }, { "codec": "opus" }, { "codec": "ac3" }],
  "subtitles": ["webvtt", "srt"],
  "max_bitrate": 20000000
}
```

Android builds it from `MediaCodecList`; the desktop player declares what its
decoder supports. The server picks, in order:

1. **Direct play**: container, video, audio all supported and the file's
   bitrate under `max_bitrate`. The plan returns `videos/{id}/file`.
2. **Remux**: codecs supported, container not (typically MKV on a client that
   wants MP4/HLS). ffmpeg copies the streams into HLS fMP4. Cheap.
3. **Audio transcode**: video supported, audio not (DTS, TrueHD, 7.1 to a
   stereo phone). Video copied, audio to AAC or Opus.
4. **Full transcode**: video codec unsupported or bitrate too high. Video to
   H.264 (universally decodable), audio to AAC, at a resolution/bitrate ladder
   capped by `max_bitrate`.

Picture-based subtitles (PGS, VobSub) that the client cannot render would
force a burn-in transcode; that is deferred, and the first versions simply do
not offer those tracks for burn-in.

### 6.2 Transcode sessions

- A session is one ffmpeg child process writing HLS fMP4 segments (6 s) into
  `<cache_dir>/sessions/<sid>/`. The server serves the playlist and segments
  from there, waiting briefly for a segment that is about to appear.
- **Seeking** past what has been produced restarts ffmpeg with `-ss` at the
  segment boundary and keeps the segment numbering (the approach Jellyfin
  uses). Seeking backwards within produced segments is free.
- **Limits**: `max_transcodes` (default 2) concurrent sessions; extra requests
  get 503 with a retry hint. One session per device per video.
- **Cleanup**: sessions die on `DELETE`, after `idle_timeout` with no segment
  requests, or on device revocation. The cache directory has a size cap and
  is wiped at startup.
- **Throttling**: ffmpeg is paused (SIGSTOP/SIGCONT) when it is far ahead of
  the client's last requested segment, so a paused movie does not burn CPU.

### 6.3 Hardware acceleration

The home server is an **Intel N150** (Alder Lake-N/Twin Lake: four E-cores,
Xe graphics). Its CPU is too weak for more than about one software 1080p
transcode, but its media engine decodes H.264, HEVC (8/10-bit), VP9 and AV1
and encodes H.264 and HEVC in hardware. So hardware transcoding is the
default path, not an option:

- The image ships **jellyfin-ffmpeg**, which bundles a recent Intel iHD
  media driver and oneVPL. Debian bookworm's driver is too old to be relied on
  for this generation.
- `/dev/dri/renderD128` is passed to the container (plus the `render` group
  id), and nothing else.
- Default config is `transcode.hwaccel = "qsv"` (with `"vaapi"` and `"none"`
  as alternatives). The whole pipeline stays on the GPU: hardware decode,
  scale with `vpp_qsv`/`scale_vaapi`, encode with `h264_qsv`.
- HDR-to-SDR tone mapping for HDR sources is done on the GPU (`vpp_qsv` tone
  mapping or OpenCL), since the CPU cannot keep up.
- `max_transcodes` defaults to 2, and that default is checked against the
  N150 during M3.
- Startup runs a short self-test encode. If the device is missing it falls
  back to software and logs a loud warning that only one transcode is
  realistic.
- Because transcoding is the scarce resource here, the decision in §6.1
  prefers remux and audio-only transcode whenever possible.

### 6.4 ffmpeg containment

- Spawned with an argument vector, never a shell; inputs passed as
  `file:/absolute/path` (a path the walker or the path jail produced) so a filename can never be read as a protocol
  or option.
- `-protocol_whitelist file`, so no network access. ffmpeg also gets
  `-nostdin`; ffprobe has no such flag, so its stdin is closed instead.
- Runs under the same unprivileged uid, with the media mount read-only and
  only its session directory writable. ffprobe gets a wall-clock watchdog and
  the container's memory limit; transcoding ffmpeg adds per-process `RLIMIT`s.
- ffprobe output is parsed with serde into strict types; unknown values are
  ignored, never interpolated into later command lines unescaped.

## 7. Clients

### 7.1 Android

Based on the emusic Android app (Compose, Material3, minSdk 26, UniFFI core):

- Keep: build wiring, server list and pairing screen (plus QR scanning via
  CameraX + ML Kit or ZXing), two-pane responsive shell, theming.
- Replace: the library becomes a folder browser (breadcrumb + list, video
  rows show duration, resolution and codec badges), fed page by page from
  `folders/{id}`.
- Player: Media3 ExoPlayer with `media3-exoplayer-hls`, Compose
  `PlayerSurface`, a `MediaSessionService` for background audio and
  notification controls, picture-in-picture, audio and subtitle track
  pickers, resume position.
- Auth: one long-lived `MobileCore` instead of one per data source; the
  `DataSource.Factory` adds a fresh bearer token per request (as emusic does)
  for both direct play and HLS segments.
- Capability profile from `MediaCodecList` (decoders, max sizes, HDR
  support).
- Tests: unit tests for the capability builder and browser model,
  instrumentation tests for pairing and playback against a fake server, same
  CI layout as emusic (APK on every PR, emulator nightly).

### 7.2 Desktop (XUI, Windows and macOS)

XUI (`va1erian/xui`) already provides the window, widgets, theming and dark
mode on both targets: the Win32 backend on Windows and the `winit` +
`tiny-skia` canvas backend on macOS. It has no video widget, no audio output
and no HTTP stack, so the desktop app needs:

- **App shell**: server list and pairing dialog, folder browser
  (`TreeView`/`ListView`), video details pane, player window. HTTP and auth
  run on a worker thread through `crates/client`, results come back to the UI
  through XUI's `Proxy`.
- **Video widget (`crates/xui-video`)**: the main piece of new work. The
  proposed route is **libmpv's render API driven from an XUI `GlWidget`**:
  - XUI's canvas and Win32 backends both host `GlWidget`s (OpenGL 3.3 through
    `xui-gpu`). mpv renders each frame into the widget's GL framebuffer.
  - libmpv brings demuxing, hardware decoding (D3D11VA/DXVA2 on Windows,
    VideoToolbox on macOS), audio output, A/V sync, HLS, subtitles and custom
    HTTP headers (for the bearer token). Writing that from ffmpeg + cpal
    would be most of the project.
  - Our wrapper owns the small `unsafe` FFI module (`ffi/`, every block with
    a `// SAFETY:` comment), following the emusic/win32ui rules. Candidate:
    the `libmpv2` crate, or a thin hand-written binding.
  - Controls (seek bar, play/pause, volume, track menus, fullscreen) are
    ordinary XUI widgets laid over or under the video node.
- **XUI work needed** (to be done in the xui repo):
  1. Today a `GlWidget` frame is rendered to a texture, **read back to the
     CPU** and composited by the software painter. That is fine for a
     visualizer but expensive at 1080p60 and above. XUI needs a direct
     presentation path for a GL node (render straight to the window's
     default framebuffer, or composite on the GPU) so video stays on the GPU.
  2. A frame-pacing hook (a "frame requested" callback tied to vsync or at
     least a high-resolution timer) so the widget can redraw when mpv signals
     a new frame instead of polling.
  3. Fullscreen and "keep screen awake" on both backends.
  4. macOS: OpenGL is deprecated but functional; if it becomes a problem, a
     Metal path is the long-term answer and should be kept in mind when
     designing (1).
- **Rejected alternative**: `ffmpeg-next` decode to RGBA on a
  worker thread, `Image::from_rgba` into a custom-painted node, `cpal` for
  audio, our own A/V sync. Simpler dependency story, far more code, CPU-bound.
- **Packaging**: libmpv shipped next to the executable (DLL on Windows, dylib
  in the `.app` bundle on macOS), dynamically linked to respect its LGPL
  build. Code signing and notarization for macOS later.

## 8. Deployment

- Multi-stage Dockerfile: `rust:1-bookworm` builder, `debian:bookworm-slim`
  runtime with jellyfin-ffmpeg (§6.3) and ca-certificates; uid 10001; port
  8080.
- Volumes: media read-only at `/media/videos`, a named data volume
  (`/var/lib/netvideo`, DB + server key + audit log), and a cache volume
  (`/var/cache/netvideo`) for transcode segments. Root FS read-only, tmpfs
  `/tmp`, `cap_drop: ALL`, `no-new-privileges`, `devices:
  /dev/dri/renderD128` with `group_add` for the render group.
- Cosmos labels (`cosmos-cloud.enabled`, `domain`, `target-port=8080`) and
  `NETVIDEO_TRUSTED_PROXIES` set to the Cosmos network. Cosmos route settings
  must not buffer responses and must allow long-lived requests; the docs will
  spell out the exact options once tested.
- Image published to GHCR by a workflow, as for emusic.

## 9. Milestones

Each milestone ends with working software and passing checks (`cargo fmt`,
`clippy -D warnings`, `cargo test`, `cargo deny`; Android `assembleDebug` +
unit tests).

| # | Milestone | Done when |
|---|---|---|
| M0 | **Scaffold** | Workspace, CI, AGENTS.md with emusic's rules, cargo-deny, empty crates build. |
| M1 | **Server core** | Auth/pairing/devices ported with roles, scanner + ffprobe, folder browsing API, direct play with Range, Docker image runs behind Cosmos. Security doc written. |
| M2 | **Android browse + direct play** | Pair (code, then QR), browse folders, play files the phone supports in ExoPlayer, resume position. Keystore-backed credentials. |
| M3 | **Transcoding** | Capability profiles, playback plans, remux and audio/full transcode to HLS, seeking, session limits and cleanup. Android plays everything. |
| M4 | **Desktop app** | XUI app on Windows and macOS: pair, browse, play via libmpv in a `GlWidget` (readback path acceptable at first). |
| M5 | **XUI video path** | Direct GL presentation and frame pacing in xui; desktop 4K playback without CPU readback. |
| M6 | **Polish** | Thumbnails (ffmpeg frame grab, cached), subtitle extraction to WebVTT, hardware transcoding, file watcher, watched/progress markers. |

M2 and M4 can run in parallel once M1's API is stable.

## 10. Testing strategy

- Server: unit tests per module (path jail, rate limiter, capability
  decision table, ffprobe parsing from recorded JSON fixtures), integration
  tests over the axum router with an in-memory DB (as emusic's
  `tests/api.rs`), and an end-to-end test that generates tiny test videos with
  ffmpeg (`testsrc`, a few seconds, h264/hevc/mkv/mp4) and checks direct play,
  remux and transcode outputs. Tests needing ffmpeg skip when it is absent.
- Fuzzing: Range header parser, path resolution, playlist/session id parsing
  (cargo-fuzz).
- Clients: see §7; the desktop widget gets headless snapshot tests of its
  controls through XUI's `OffscreenBackend`.

## 11. Decisions taken

- **Desktop decoder**: libmpv's render API in an XUI `GlWidget` (§7.2).
- **Server hardware**: Intel N150, so jellyfin-ffmpeg with QSV/VAAPI is the
  default transcoding path (§6.3).
- **Accounts**: one owner, many devices; watch progress is per device.
- **Code from emusic**: copied and adapted into this repo. Extracting a shared
  auth crate can be revisited once netvideo's auth changes settle (after M2).

## 12. Open questions

1. **Where the video widget lives**: in netvideo (`crates/xui-video`,
   proposed at first) or in the xui repo from the start?
2. **Resume position and watched markers**: taken in M2, as proposed; the
   server stores them per device (`PUT videos/{id}/progress`).
3. **QR pairing**: worth it for M2, or keep code-only pairing until later?
4. **Remote bandwidth**: should the server offer an adaptive multi-bitrate
   ladder, or one rendition chosen from the client's `max_bitrate`
   (proposed, simpler)?
