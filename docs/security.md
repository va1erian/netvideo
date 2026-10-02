# netvideo security model

This document describes what netvideo protects, from whom, and how. It covers
the server as built today; planned items are marked **(planned)** with the
milestone that brings them. The design rationale lives in
[PLAN.md §5](PLAN.md#5-security).

## Threat model

netvideo runs on a home server and is reachable from the internet through
Cosmos Cloud. Anyone can send requests to the API; only paired devices may
use it.

Assets, most valuable first:

1. **The server signing key** (`server.key`). Whoever holds it can mint
   tokens for any device.
2. **Device private keys and tokens.** Each one grants that device's access
   until it is revoked.
3. **The media files and their names.** These are private to the owner.
4. **Server availability**, on a weak Intel N150.

Attackers considered:

- **An internet stranger.** They can reach the API but have no pairing code
  and no device key.
- **A hostile network** between a client and the server, for example public
  Wi-Fi.
- **A paired viewer device** that tries to do more than browse and play.
- **A malicious media file.** Filenames and contents are parsed by ffprobe
  (and later ffmpeg), so they count as untrusted input even though the owner
  put them there.

Out of scope:

- An attacker with a shell on the host or root in the container.
- A compromised Cosmos Cloud.
- An owner who hands out admin codes freely.

## Pairing

- `netvideo-server pair` prints a 6-digit, single-use code that is valid for
  `pairing_code_ttl_secs` (default 300 s, at most 3600 s).
- The database stores only an HMAC of each code, keyed by the server secret.
  A leaked database therefore does not reveal live codes.
- Wrong, used and expired codes all get the same 401, so a guesser learns
  nothing from the response.
- Guessing is rate limited at three levels:
  - per client IP, with IPv6 clients grouped by /64;
  - globally, at 30 failed attempts per minute across all clients, which
    bounds a distributed guess against the 10⁶ code space;
  - fail closed: if the limiter's table is full, requests are refused, not
    let through.
- The device generates its own Ed25519 key and sends only the public half.
  The server never sees a device private key.
- **(planned, M2)** A QR pairing code will also carry the server URL and the
  fingerprint of the server's public key. The client then rejects a token
  signed by any other key, which defeats a man-in-the-middle during pairing.

## Tokens and devices

- Access tokens are PASETO v4.public, signed with the server key and valid
  for `token_ttl_hours` (default 168).
- To refresh a token, the client must send a proof signed by its device key
  that names the current token's fingerprint. A stolen token can be used
  until it expires but cannot be extended.
- Every authenticated request checks that the device still exists and is not
  revoked, so revocation takes effect immediately.
- **Roles.**
  - Devices paired from a CLI code are admins, unless the code was minted
    with `--viewer`.
  - Codes minted through the API create viewers unless an admin asks for an
    admin code.
  - Listing devices, minting codes, revoking devices and starting scans all
    require an admin. A viewer gets 403.
- The server key is generated on first start and stored as
  `<data_dir>/server.key`, mode 0600, outside the database.

## Network edge

- `X-Forwarded-For` is honoured only when the direct peer is inside
  `trusted_proxies` (the Cosmos bridge). Otherwise the peer address is the
  client address, so a client cannot spoof its IP to escape rate limits.
- TLS is terminated by Cosmos, or by the server itself when
  `tls_cert`/`tls_key` are set. Configuring only one of the two is rejected
  at startup.
- **Stream limits.** A device may hold at most 4 file streams at once, and
  the server at most 32 in total. Extra requests get 429. Each stream holds a
  file and a socket, and a client that stops reading holds both, so without
  these limits one device or stolen token could exhaust the server's file
  descriptors. The compose file also raises `nofile` to 8192.
- Request bodies are capped at `max_body_bytes`, and unknown JSON fields and
  unknown query parameters are rejected.
- **(planned, M2)** The Android client will refuse cleartext traffic, except
  in explicit LAN debug builds.
- **No secrets in URLs.** Tokens travel only in the `Authorization` header.
  **(planned, M3)** HLS segment requests will also use that header, so
  playlists carry no tokens.

## File access

Clients only ever name database ids. They never name paths.

- **Scanning.**
  - Library roots must be absolute paths, and each may be listed only once.
  - The walker does not follow symlinks and skips hidden entries, including
    names that are not valid UTF-8. Other paths that are not valid UTF-8
    cannot be shown by the API and are skipped with a warning.
  - A root that is unreachable, or only partly readable, never causes rows to
    be pruned, so a flaky mount cannot wipe the library. A folder (or the
    root) that now has no entries at all, while the database still knows
    videos under it, is treated the same way, because an unmounted share or
    a missing bind mount looks exactly like an empty directory. A folder that still holds
    anything (subtitles, posters, hidden files) is pruned as usual, and so
    is a folder deleted outright. To retire videos on purpose, delete their
    folder rather than just emptying it.
  - Rows are tied to each root's path, not its position in the config.
    Reordering or removing roots moves or deletes their rows at startup,
    before anything is served, so an old video id can never resolve under a
    different root.
- **Serving.**
  - Every file read resolves the stored relative path through the path jail:
    absolute paths and `..` are rejected, the path is canonicalised, and it
    must stay under the canonical root and be a regular file.
  - An escape returns 404, the same as a missing file, and is written to the
    audit log. A plain missing file is not logged as an attack.
  - The file is opened non-blocking, so a FIFO swapped in cannot hang the
    server, and only once. The open handle's real location is checked
    again (`/proc/self/fd` on Linux) before any byte is read. Metadata and
    content both come from that handle, so a directory swapped for a symlink
    after resolution cannot leak files from outside the root.

## ffprobe and ffmpeg containment

- The program is spawned with an argument vector, never through a shell.
- The input is passed as `file:<absolute path>`, together with
  `-protocol_whitelist file`, so a filename can never be read as an option or
  as a network URL.
- `-format_whitelist` limits ffprobe to the container demuxers netvideo
  indexes. Playlist (HLS) and concat demuxers, which can pull in other
  files, are excluded.
- stdin is closed and stderr is discarded.
- Each probe has a 60 s wall-clock limit. stdout is capped at 4 MiB while it
  is read, so the cap bounds memory, and the child is killed when it
  overflows or times out.
- At most two probes run at once, which keeps the N150 usable during a scan.
- A file ffprobe fails on is marked, and is only probed again once its size
  or mtime changes. A hostile file therefore costs one probe, not one per
  scan.
- The probe's memory is not limited per process. The compose file's
  `mem_limit` bounds the container instead, so a runaway ffprobe gets the
  container OOM-killed and restarted rather than starving the host.
- Output is parsed into strict serde types, free-form strings are capped at
  200 characters, and nothing from a probe is ever put back on a command
  line.
- **(planned, M3)** Transcoding ffmpeg processes will get the same treatment,
  plus a per-session scratch directory as the only writable path and
  per-process resource limits.

## Residual risks

These would need write access to the media, which only the owner's other
machines have, or are accepted for now:

- A broken ffprobe install that cannot even print its version is treated as
  missing, so it does not mark the library unreadable. A transient start
  failure (EMFILE, EAGAIN) is retried on the next scan.
- ffprobe opens the walked path directly. A directory swapped for a symlink
  between the walk and the probe could let ffprobe read metadata from
  outside the root. Serving is not affected, because it re-checks the
  opened handle.
- Revoking a device stops new requests at once, but a stream that is
  already open runs until the client closes it, at most 4 per device.
- There is no idle timeout on a stream yet; the stream limits bound how
  many can be held open.

## Container

- The container runs as uid 10001 with a read-only root filesystem,
  `cap_drop: ALL` and `no-new-privileges`.
- The media is mounted read-only. The only writable volume is the data
  directory (database, server key, logs).
- The port is `expose`d to the Cosmos network only and not published on the
  host.

## Audit log

- `<data_dir>/audit.log.<date>` (one file per day) records JSON lines for:
  - pairing successes and failures;
  - token refreshes;
  - revocations;
  - authentication failures;
  - rate-limit hits;
  - path escapes;
  - finished scans.
- `RUST_LOG` filters only the console output. The audit log always records
  security events.

## Supply chain

- CI runs `cargo fmt`, `clippy -D warnings` and the tests with `--locked`,
  plus RustSec `audit-check` for known-vulnerable or yanked dependencies.
- **(planned)** `cargo deny` for licences and sources, pinned image digests,
  and Dependabot.

## Reporting a problem

Open a private security advisory on the GitHub repository rather than a
public issue.
