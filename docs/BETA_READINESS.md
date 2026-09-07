# First beta work log

Baseline: 6408793 (alpha.3). Audit started 2026-09-06.

## Scope
Review protocol parsing and networking, storage and durable state, runtime lifecycle, native packaging, browser UI, and automated release coverage. Implement and verify concrete findings, with honest boundaries for unsupported protocol features and external services.

## Corrections

### Peer interoperability and runtime

- Corrected the MSE Diffie–Hellman prime. The old implementation used the Oakley value instead of the BitTorrent MSE value. Self-roundtrip tests shared the mistake; Transmission with required encryption exposed it. Verified against [libtorrent's implementation](https://github.com/arvidn/libtorrent/blob/RC_2_0/src/pe_crypto.cpp).
- Incoming TCP sockets explicitly enter blocking mode before timeout-based peer handling. On macOS, inheriting nonblocking mode caused immediate idle-loop disconnection.
- Incoming and outgoing peers now use the same bidirectional transfer loop, shared memory budgets, and configured global/per-torrent connection slots.
- The client serves verified info-dictionary metadata to magnet peers. Extension IDs follow their direction: send using the peer's advertised ID, receive using Rustorrent's own ID. PEX uses the same rule. Extension ID zero disables a capability. See [BEP 10](https://www.bittorrent.org/beps/bep_0010.html) and [BEP 9](https://www.bittorrent.org/beps/bep_0009.html).
- MSE accepts empty/partial initial payloads and preserves following protocol bytes. Encrypted handshakes beginning with byte 19 are distinguished using the complete protocol prefix.
- Endgame piece selection yields to incoming reads instead of spinning on an existing reservation. The request pipeline no longer duplicates the same outstanding block to the same peer.
- The console progress worker releases the logging lock before state updates and sleeping. Metrics remain updated in daemon/TUI mode.
- Per-torrent stop cancels bandwidth waits promptly, and changing a rate limit wakes a wait using the previous limit.

### State and files

- Start-paused and file-selection options are part of the add command and applied before transfers start. Invalid metadata and out-of-range file selections are rejected before queueing.
- Paused state is durable; a stopped transfer restarts paused. Manual resume clears the saved flag.
- Resume recovery rechecks files changed since the snapshot, recovering verified pieces written immediately before a crash.
- Preallocation rejects an existing file larger than the torrent payload instead of truncating it. Every opened file and its identity is validated before resizing begins.
- Add commands cap library/queue capacity. Existing descriptor-relative path, ownership, backup, tombstone, rename, proxy, and input-budget protections remain in place.

### Router mapping

- NAT-PMP renewal uses the gateway's granted lifetime and rejects zero-lifetime mappings. UPnP is periodically refreshed. Failed mappings retry with bounded backoff.
- UPnP derives the local client address from the route to the actual gateway. Its service type is restricted to recognized WAN connection services.
- Mapping does not report success when the incoming listener failed to open. `--no-port-mapping` lets tests and users disable router changes.
- Ad-hoc bundle signing remains an integrity measure, not proof that macOS Local Network permission will persist across rebuilds. The packaging comment now states this limitation accurately.

### Interface

- Replaced the oversized statistics overview with a compact transfer workspace, clear waiting/seeding messages, and progressive session/RSS controls.
- Added visible per-torrent errors, no-match filtering, inline add errors, a reconnection notice, and one explicit removal dialog that keeps files by default.
- Live updates preserve keyed transfer identity, focused edits, and opened disclosures. Dialogs support keyboard focus, Escape, and focus restoration.
- Added light/dark contrast checks, labels, reduced-motion handling, narrow-screen layout checks, and fallback removal-dialog behavior for older WebKit.
- File previews enforce byte/depth/node budgets, reject duplicate/prototype dictionary keys, and support v2 file trees.
- CSS and JavaScript now live in editable assets compiled into the same standalone executable. Node and Playwright are development dependencies only.
- The native app keeps UI preferences between launches and supports smaller windows.

## Validation

- Full Rust suite: 454 unit/local tests, 9 adversarial process tests, and 17 short transport tests passed locally. Minimal suite: 358 unit/local tests plus the process and transport suites passed.
- Eight independent Python wire/process scenarios cover complete transfer, upload, multi-chunk magnet metadata with asymmetric extension IDs, corrupt peer data, abrupt mid-transfer death, paused selection, safe preallocation, and stop during bandwidth throttling.
- Transmission 4.1.3 interoperability covers downloading from Transmission, uploading to Transmission, and uploading with encryption required by Transmission. Payload bytes are compared independently.
- Five real-browser scenarios exercise add, restart, pause/resume, filtering, live edits, removal, desktop/mobile layout, and WCAG A/AA automated checks in light/dark mode.
- A 120-second TCP/uTP transport soak passed. This is a transport soak, not a claim of a multi-hour torrent swarm test.
- Five fuzz targets each ran 2,000 coverage-guided inputs without a sanitizer. No crashes were observed in these finite smoke runs.
- Rust 1.89 and strict all-feature Clippy checks passed. Both dependency lockfiles passed cargo-audit. Cross-platform CI and final artifact validation are required before publication.
- The adversarial process harness now uses file-backed logs and RAII child cleanup, and allocates distinct ports across parallel tests. This fixes output-pipe stalls, leaked children on failed assertions, and port-reuse collisions in the test infrastructure.

## Beta boundaries

The tested release path is v1 torrents and v1 magnets on macOS/Linux, with Windows runtime coverage in CI. v2/hybrid parsing and verification remain available, but v2/hybrid magnets need a complete HTTP metainfo source and discovery does not independently join both hybrid swarms. These are documented protocol limits, not newly completed features.

Search plugins are third-party Python programs running with the user's privileges. Public swarms, third-party plugins, consumer routers, and network permissions vary beyond deterministic fixtures. Automated accessibility checks do not replace assistive-technology testing.

There is no Developer ID certificate on this machine, so the candidate is ad-hoc signed and is not Apple notarized. Multi-hour torrent soaks, sustained sanitizer campaigns, and physical Windows/Linux ARM testing are not claimed by this pass.
