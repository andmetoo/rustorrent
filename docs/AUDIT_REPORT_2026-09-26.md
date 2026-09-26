# Audit, interface and size pass — 2026-09-26

Baseline: `6d5fa19` (0.1.0-beta.1). This pass reviewed the whole codebase for correctness, crash
safety and throughput, redesigned the web interface, brought the command line up to the web
interface's feature set, and reduced the binary size.

## Size

| Build (Linux x86_64, release) | Before | After |
|---|---|---|
| Default features | 2,227,320 B | 1,639,008 B (−26%) |
| `--no-default-features` | 1,744,760 B (with abort) | ~1,430,000 B |

The main reductions, including the new command-line features:

| Change | Approximate saving |
|---|---|
| `panic = "abort"`: no unwinding machinery | 290 KB |
| Client-rendered web UI instead of server-built HTML, with assets gzipped by `build.rs` and served compressed | 185 KB |
| Engine deduplication: one non-generic worker spawner, data-driven command dispatch, shared teardown and persistence helpers | 110 KB |
| `num-bigint`/`num-traits` replaced by in-house 768-bit arithmetic; trackers share one HTTP client and resolver | 105 KB |
| One small shared heapsort instead of a standard sort instantiated per call site | 60 KB |
| Search runtime stored gzip-compressed, plus storage, RSS and search trims | 40 KB |

**The 1 MB target is not met.** About 345 KB is fixed standard-library cost: panic and backtrace
support, formatting and float parsing. On Linux about 200 KB more is `.eh_frame` unwind tables,
which stable Rust always emits. Reaching 1 MB would mean removing major features, not further
optimisation. With every optional protocol disabled the build is still 1.4 MB. macOS builds carry
compact unwind info instead of `.eh_frame`, so they are expected to be smaller, but they were not
measured here.

Because release builds now abort on panic, the review treated every reachable panic on network,
disk or plugin input as a crash bug and removed the ones it found.

## Correctness and security fixes

### Protocol and network

**uTP**
- Connection ids and sequence numbers were inverted relative to BEP 29 and libutp, so uTP only
  worked between rustorrent instances. This is intentionally incompatible with older rustorrent
  builds' uTP.
- Writes waited for an ACK after every packet, limiting throughput to about 25 KB/s.
- ACKs carried on DATA and FIN packets were ignored.
- The sender could exceed the peer's advertised window by one packet.
- LEDBAT rounding froze the congestion window at 4 packets.
- A FIN could truncate data that had arrived out of order.
- SYN retransmission and keepalives were missing.

**DHT**
- Each torrent sent a single `get_peers` to one arbitrary node, which could neither find peers
  nor make us findable. It now runs proper iterative lookups and announces to the closest nodes
  that returned tokens.

**Torrent parsing and trackers**
- Every top-level key of a torrent file got its own allocation budget, allowing
  memory-amplification attacks.
- A single port-0 peer entry discarded a whole tracker response.
- A `#fragment` in an announce URL swallowed the query string.
- The HTTP client had an overflow in a size calculation, accepted a bare `:port` as a host, and
  accepted `+`-signed status codes and chunk sizes.

**LPD**
- We announced ourselves to ourselves as a peer.
- The IPv6 Host header was wrong.
- In multi-infohash messages only the last hash counted.
- Link-local peers lost their scope.

**Clock arithmetic**
- `Instant::now() - duration` panicked when the process started shortly after boot, which would
  crash-loop under launchd or systemd.

**Peer discovery**
- Known-peer, ban and PEX input grew without limit.

**Transmission interoperability**
- Transmission 4.0.5 occasionally loses a bitfield that arrives right after an encrypted
  handshake, then never becomes interested. This reproduced on the baseline in about 1 run in 10.
- Seeds now repeat their pieces once as HAVE messages to peers that remain uninterested. After the
  fix, 30 of 30 runs passed.

### Engine lifecycle

**Removing and stopping**
- A magnet fetching metadata, or a torrent still verifying, could not be stopped, paused, archived
  or removed. Loading is now cancellable.
- Deleting a queued duplicate of an active torrent could delete the active torrent's files.
- Stopping a queued torrent did not persist the paused state. Failed or stopped-before-start
  entries could be neither resumed nor removed.

**Scheduling**
- Four seeding or paused torrents blocked every further download under the default
  `--max-active 4`.
- The seed-ratio limit stopped downloads as well as seeding torrents.
- The tracker announce interval could only ever shrink.

**Peer handling**
- Pieces partially received from another peer could never finish, stalling downloads near the end.
- Our own keepalives kept idle peers alive forever.
- BEP 6 reject-request messages were ignored.
- Unchoking was effectively random, because rates were re-ranked from millisecond samples on every
  interest change.
- Torrents started together shared a peer id.

**Watch folder**
- A `.torrent` still being copied into the watch folder was read truncated and discarded.

### Storage, state, search and RSS

**Storage and state**
- Torrents with a few hundred files failed to start on the default open-file limit (256 on macOS).
  The soft limit is now raised.
- Disk-full errors during preallocation were only recognised on Unix; Windows error codes are now
  mapped too.
- A malformed file layout could overflow while mapping a read or write to files.
- A read limit in the state directory could overflow.

**Search**
- Downloads through a search plugin never worked: the plugin's temporary file was always rejected
  by the containment check.
- Output paths containing spaces were cut at the first space.
- Windows plugins could not open sockets because `SYSTEMROOT` and related variables were not
  passed.
- Catalog text with non-ASCII characters was garbled.
- A search for `--capabilities` switched the runner into capabilities mode.
- Mixed-case engine names never ran.
- One failing engine aborted the remaining engines.

**IP blocklists**
- PeerGuardian and eMule blocklists could not be loaded.

### Terminal interface
- Raw mode wrote termios fields at Linux offsets, which corrupted `c_cflag` and the wrong control
  characters on macOS. It now uses `libc::termios`.

## Performance

| Measurement | Before | After |
|---|---|---|
| SHA-1, 256 MiB | 192 MB/s | 1,546 MB/s with SHA-NI; 460 MB/s portable |
| uTP loopback, 2 MiB | 83 s | 0.54 s |
| Download 256 MiB from 4 seeders | 135 MB/s | 235 MB/s |
| Download 1 GiB (256 KiB pieces) from 4 seeders | 48 MB/s | 164 MB/s |
| Upload 256 MiB to 4 leechers | 210 MB/s | 675 MB/s |
| 2,000 piece picks over 50,000 pieces | 5.9 s | 37 ms |
| Writing a 4,000-file torrent with cache and flush | 1.29 s | 0.53 s |

**Peer loop**
- Reads 64 KiB at a time and handles every buffered message before housekeeping.

**Stalls removed**
- Tracker announces no longer block the torrent loop.
- Resume saves no longer hold the piece lock while syncing files.
- A manual recheck no longer freezes peers.

**Web interface**
- The page receives JSON deltas instead of the full page HTML every 450 ms.

**Fewer disk syncs and less startup work**
- Only files written since the last sync are fsynced.
- Idle resume saves are skipped.
- Unchanged session entries are no longer rewritten at startup.

## Interface

**Web interface**
- Redesigned as a modern, quiet workspace: a sidebar with filters, labels and tools; a transfer
  list with state pills, slim progress bars and hover actions; and rows that expand into Files,
  Trackers, Peers and Info.
- Also added: drag-and-drop and paste-anywhere adding, toasts, keyboard shortcuts, light and dark
  themes that follow the system by default, and a narrow-screen layout. The content security
  policy is stricter.

**Command line**
- `rustorrent remote <command>` controls a running instance with the web interface's full feature
  set: adding, lifecycle actions, files, labels, trackers, limits, seed ratio, peer profile,
  search, plugins and RSS, with `--json` output.
- `--tui` offers the same actions interactively and can attach to a daemon through
  `rustorrent remote tui`.
- `--help` and `--version` were added.

## Validation

**Automated suites (Linux)**
- 515 unit tests, 9 process tests and 20 transport tests pass with all features; 411 unit tests
  pass in the minimal build.
- 8 real-process transfer scenarios pass.
- All 3 Transmission interoperability scenarios pass, including 30 consecutive runs of the
  encrypted upload.
- 9 browser flows pass, including axe WCAG A/AA checks in light, dark and narrow layouts.

**Lints and compile checks**
- `clippy -D warnings` passes on Linux, and for the macOS and Windows targets.
- Each optional feature builds on its own.
- The build compiles with Rust 1.89.
- The fuzz targets lint cleanly.

**Not covered**
- Runtime testing on macOS and Windows, a signed app bundle, and long-running public-swarm soaks
  were not part of this pass.
