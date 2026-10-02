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

- Pairing codes have 6 digits and are single-use.
  - `netvideo-server pair` prints one valid for `--ttl` seconds (default 600,
    capped at 3600).
  - Codes minted through the API are valid for `pairing_code_ttl_secs`
    (default 600, at most 3600). An admin may mint 10 per minute per IP.
- The database stores only an HMAC of each code, keyed by the server secret.
  A leaked database therefore does not reveal live codes.
- Wrong, used and expired codes all get the same 401, so a guesser learns
  nothing from the response.
- Guessing is rate limited at three levels:
  - per client IP, `max_pairing_attempts_per_min` attempts (default 3,
    successes included), with IPv6 clients grouped by /64;
  - globally, at 30 failed attempts per minute across all clients, which
    bounds a distributed guess against the 10⁶ code space. Each attempt is
    charged before it runs, and refunded only if it succeeds or the per-IP
    limit refuses it, so concurrent guesses cannot overrun the budget. The
    trade-off is that a guesser can block pairing for everyone for a minute at a time;
    already paired devices are unaffected;
  - while the limiter's table is full (4096 tracked clients), new client
    addresses are refused rather than let through.
- The device generates its own Ed25519 key and sends only the public half.
  The server never sees a device private key.
- **Server key pinning.** The pairing reply carries the server's public
  key, and the client pins it with its credentials. Every access token
  names the device it was issued to (`sub`) and the fingerprint of that
  device's public key (`device_key`). The client checks every token it
  receives, at pairing and on each refresh: signed by the pinned key, for
  its own device id, for its own key. A token that fails stops the session
  with an identity error, rather than falling back to the current token.
  Credentials from before pinning existed pin the key the server sends at
  their next refresh.
- **QR pairing.** `netvideo-server pair --qr --url <address>` also prints a
  QR code holding the address, the code and the fingerprint of the
  server's public key. A client that scans it refuses a server whose key
  does not match, before it stores anything. This stops an impostor that
  does not hold the server key (a hijacked address, a lookalike server)
  from pairing the device.
  - What it does not stop: the code travels to the server in the clear
    inside TLS. A man-in-the-middle that breaks TLS (a rogue certificate)
    and can reach the real server can redeem the code itself, or pair its
    own key in the device's place. The second case is caught: the reply's
    `device_key` names the attacker's key, so the device refuses it and
    reports an identity error instead of believing it is paired. TLS stays
    the defence for the code itself.
  - A typed code has no fingerprint: the first key the client sees is
    trusted, then pinned.
  - The QR code contains the live pairing code, so it is as sensitive as
    the code itself.
- The HMAC key is the server's signing key, so pairing adds no secret of its
  own to protect.

## Tokens and devices

- Access tokens are PASETO v4.public, signed with the server key and valid
  for `token_ttl_hours` (default 168).
- To refresh a token, the client must send a proof signed by its device key
  that names the current token's fingerprint. A stolen token can be used
  until it expires but cannot be extended.
- A proof must expire within 10 minutes of being checked, and within
  15 minutes of its own `iat` (10 plus a clock-skew allowance). Each proof
  is accepted once: the server remembers used proof ids until they expire,
  at most 8 live ones per device, so one device cannot crowd out the
  others' refreshes. A refresh does not revoke the old token, which stays
  valid until its own expiry.
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
  `<data_dir>/server.key`, created with mode 0600 and kept outside the
  database. It shares the data volume with the database, so a backup of
  that volume holds both and must be protected like the key.

## Network edge

- `X-Forwarded-For` is honoured only when the direct peer is inside
  `trusted_proxies`. Otherwise the peer address is the
  client address, so a client cannot spoof its IP to escape rate limits.
  IPv4 peers that a dual-stack listener reports as `::ffff:a.b.c.d` are
  matched as IPv4.
- The example compose file trusts `172.16.0.0/12`. Docker isolates bridge
  networks from each other, so only containers on netvideo's own network,
  and processes on the host (through the bridge gateway), can use that
  trust. Narrow it to the exact address of Cosmos on that network when
  other containers or untrusted host users are around.
- TLS is terminated by Cosmos, or by the server itself when
  `tls_cert`/`tls_key` are set. Configuring only one of the two is rejected
  at startup.
- **Stream limits.** A device may hold at most 4 file streams at once, and
  the server at most 32 in total. Extra requests get 429 before any file is
  opened, and are audited. Each stream holds a
  file and a socket, and a client that stops reading holds both, so without
  these limits one device or stolen token could exhaust the server's file
  descriptors. The compose file also raises `nofile` to 8192.
- **Timeouts.** A connection must send each request's headers within 30 s
  of opening or of going idle, which also closes silent and idle
  keep-alive connections. Every request other than a file stream must
  finish within 30 s, body included. The server speaks HTTP/1.1 only, with
  ALPN `http/1.1` under TLS: detecting HTTP/2 would mean waiting on a
  silent connection, and HTTP/2 connections get no header timeout.
- Request bodies are capped at `max_body_bytes` (default 64 KiB, at most
  1 MiB). JSON bodies and folder-listing queries reject unknown fields.
  Endpoints that take no query parameters ignore the query string.
- **Client storage and transport (Android).** Release builds refuse
  cleartext, both in the platform's HTTP stack and in the Rust core's own
  sockets; debug builds allow it for LAN testing. The device key and
  tokens are sealed by a non-exportable Android Keystore key, and backups
  are disabled.
- **No secrets in URLs.** Tokens travel only in the `Authorization` header.
  **(planned, M3)** HLS segment requests will also use that header, so
  playlists carry no tokens.

## File access

Clients only ever name database ids. They never name paths.

- **Scanning.**
  - Library roots must be absolute paths, and each path may be listed only
    once (compared as written, so two spellings of one directory are not
    caught).
  - The walker does not follow symlinks and skips hidden entries, including
    names that are not valid UTF-8. Other paths that are not valid UTF-8
    cannot be shown by the API and are skipped with a warning.
  - A root that is unreachable, or only partly readable, never causes rows to
    be pruned, so a flaky mount cannot wipe the library. A folder (or the
    root) that now has no entries at all, while the database still knows
    videos under it, keeps the rows beneath it, because an unmounted share
    or a missing bind mount looks exactly like an empty directory. The rest
    of the root is scanned and pruned normally. A folder that still holds
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
    again through `/proc/self/fd` before any byte is read. (Off Linux, which
    is for development only, the path is re-resolved instead; that narrows
    the race but does not close it.) Metadata and
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
  scan. A broken ffprobe install that cannot print its version marks
  nothing, and a transient start failure (EMFILE, EAGAIN) is retried on the
  next scan.
- The probe's memory is not limited per process. The compose file's
  `mem_limit` bounds the container instead, so a runaway ffprobe gets the
  container OOM-killed and restarted rather than starving the host.
- Output is parsed into fixed serde types, free-form strings are capped at
  200 characters, and nothing from a probe is ever put back on a command
  line.
- **(planned, M3)** Transcoding ffmpeg processes will get the same treatment,
  plus a per-session scratch directory as the only writable path and
  per-process resource limits.

## Residual risks

Most serious first. The first is planned for M3; the rest are accepted, or
need write access to the media, which only the owner's other machines have.

- **ffprobe runs as the server's user.** It can read the data directory,
  including `server.key`. A parser exploit in ffprobe, triggered by a
  hostile media file, could steal the signing key and mint tokens for any
  device. M3 runs ffprobe and ffmpeg sandboxed away from the data
  directory.
- **No idle timeout on a stream.** A client that stops reading keeps its
  stream open; the stream limits bound how many it can hold.
- The proof log lives in memory, so a restart forgets it. A proof captured
  before a restart can be replayed once within its 10 minutes.
- Taking a stream slot comes first, so a device with 4 open streams gets
  429 even for a request that would have been a 304.
- ffprobe opens the walked path directly. A directory swapped for a symlink
  between the walk and the probe could let ffprobe read metadata from
  outside the root. Serving is not affected, because it re-checks the
  opened handle.
- Revoking a device stops new requests at once, but a stream that is
  already open runs until the client closes it, at most 4 per device.

## Container

- The container runs as uid 10001 with a read-only root filesystem,
  `cap_drop: ALL` and `no-new-privileges`.
- The media is mounted read-only. The writable paths are the data
  directory (database, server key, logs) and a tmpfs at `/tmp`.
- The port is not published on the host. It is reachable from containers
  on the same Docker network, which Cosmos must join.

## Audit log

- `<data_dir>/audit.log.<date>` (one file per day) records JSON lines for:
  - pairing successes and failures;
  - token refreshes;
  - pairing codes minted and devices revoked, from the CLI (`client_ip` is
    `cli`) or by an admin device, which is named;
  - authentication failures, including a viewer refused an admin action;
  - rate-limit hits, for pairing and for streams;
  - path escapes;
  - finished scans.
- `RUST_LOG` filters only the console output. The audit log always records
  security events.
- Old files are not deleted; prune them with the host's log rotation.
- Run CLI commands as the server's user (`docker exec`, as in the compose
  file, does this). A root-owned log file would stop the server from
  rolling over to the next day's file.

## Supply chain

- CI runs `cargo fmt`, `clippy -D warnings` and the tests with `--locked`,
  plus RustSec `audit-check`, which fails on known-vulnerable dependencies.
  It runs on pushes and pull requests only, not on a schedule.
- **(planned)** `cargo deny` for licences and sources, pinned image digests,
  and Dependabot.

## Reporting a problem

Report it privately to the owner rather than in a public issue.
