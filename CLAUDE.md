# CLAUDE.md
> Project memory and standing instructions for Claude Code.
> This file is loaded at the start of every session. Keep it current.
> Commit this file to version control — it is shared team infrastructure.

---

Before we begin: read this file fully before doing anything else.

---

## What this project is

A Rust daemon that proxies BLE advertisements from a Linux host's BlueZ stack
to Home Assistant, by reimplementing the Bluetooth-proxy subset of ESPHome's
native API protocol over TCP (advertised via mDNS as `_esphomelib._tcp` for
auto-discovery). Fork of `reedstrm/linux_bt_proxy`, maintained at
`k-meeks/linux_bt_proxy`. Runs in production on the maintainer's
OpenMediaVault NAS, monitoring specific BLE sensors. Passive advertisement
forwarding only (legacy + raw) — no GATT connections, pairing, or cache
clearing; talks to BlueZ entirely over D-Bus, no root or special
capabilities needed at runtime.

---

## Usage

```
cargo build --release
./target/release/linux_bt_proxy [OPTIONS]
```

Options: `-a/--hci <INDEX>` (default 0), `-l/--listen <ADDR>` (default
`[::]:6053`), `--hostname <NAME>` (default system hostname), `-m/--mac <MAC>`
(default: auto-detected). No config file — flags only.

Packaged installs run as the `linux-bt-proxy` systemd service under an
unprivileged `linuxbtproxy` system user (member of the `bluetooth` group).
See README.rst for full install/deployment instructions.

---

## BlueZ / D-Bus / ESPHome API — confirmed quirks

- `zbus` talks to D-Bus directly over the socket — no `libdbus` binding, no
  system `protoc` needed (protobuf code is generated with a pure-Rust parser
  in `build.rs`). This is why static musl linking (below) works cleanly: no
  C dependencies at all.
- BlueZ D-Bus object paths can't contain `:`, so MACs appear in paths as
  `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`. The real colon-separated `Address`
  property is still present in the property dump, so grep for either form.
- BlueZ doesn't expose raw over-the-air advertisement bytes — `raw_adv.rs`
  reconstructs the AD-structure byte stream from BlueZ's already-parsed
  `Device1` properties. This is best-effort: payload bytes (service/
  manufacturer data) are byte-exact, but framing BlueZ doesn't surface (e.g.
  the AD Flags byte) is omitted.
- `ACTIVE_CONNECTIONS` is the prerequisite gate for `PAIRING`,
  `CACHE_CLEARING`, and `REMOTE_CACHING` in HA's `BluetoothProxyFeature`
  flags — without it HA never attempts those operations regardless of the
  other bits. This proxy only sets `RAW_ADVERTISEMENTS` (`0x20`).
- BlueZ's `SetDiscoveryFilter` defaults `DuplicateData` to `false`, which
  makes the controller drop repeat advertisements with unchanged payload
  *before* `bluetoothd` (and therefore this proxy's D-Bus listener) ever
  sees them — only the first sighting of each device gets through. Fixed by
  passing `DuplicateData: true` in the filter dict (`ble.rs`,
  `try_start_discovery`).
- Adapter hardware matters: BLE requires a Bluetooth 4.0+ controller. An
  adapter can report `Discovering: yes` and `UP RUNNING` while only doing
  classic BR/EDR inquiry scanning if its chipset predates LE (e.g. the
  Broadcom BCM2070, HCI version 4 = Bluetooth 2.1+EDR). Confirm LE support
  with `btmgmt info` — look for `le` in "supported settings" — before
  debugging the proxy software itself.
- `env_logger::init()` defaults to **no output at all** when `RUST_LOG` is
  unset — even `info!()` startup lines are silent by default. Use
  `RUST_LOG=linux_bt_proxy=debug` (module-scoped) to see this crate's debug
  logs without third-party noise.
- `mdns-sd` tries to bind a multicast socket on every local interface and
  logs+skips (at debug level) any that fail — e.g. Docker bridge interfaces
  returning EPERM. This is expected, by-design behavior, not a bug.
- Home Assistant compares the `esphome_version` field we report (currently
  our own crate version, see `src/handlers.rs`) against its table of known
  real ESPHome releases. Since this isn't real ESPHome firmware, HA will
  perpetually nag to "update" with no actual update path available — known
  cosmetic issue, intentionally left alone (spoofing a fake version to
  suppress it previously caused HA registration problems, per the FIXME
  comment in `device_info_request`).
- `cargo-deb` / `cargo-generate-rpm` require the **literal string**
  `target/release/...` in `Cargo.toml` asset paths — never hardcode a
  target-triple path. Both tools substitute the real target directory
  themselves when given `--target <triple>` on the command line.
- Release binaries are built against `x86_64-unknown-linux-musl` (static
  linking) specifically because GitHub's `ubuntu-latest` runner's glibc is
  newer than many real deployment targets (e.g. Debian-based NAS distros) —
  a glibc-linked binary built there will fail with `GLIBC_X.XX not found` on
  older systems.
- `cargo-deb`'s default `.deb` filename includes a `-1` revision suffix
  (e.g. `linux-bt-proxy_0.1.1-1_amd64.deb`) — easy to forget when
  hand-writing download URLs in release notes.
- GitHub disables Actions on forks by default, with no API/CLI bypass (only
  a manual one-time opt-in via the repo's Actions tab). This repo's default
  `GITHUB_TOKEN` permissions are also read-only, so `release.yml` needs an
  explicit job-level `permissions: contents: write` rather than relying on
  the repo-wide default.

---

## "This One Goes to Eleven"

When presenting numbered lists of options to the user, the count must never
be exactly 10. Fewer than 10 is fine. More than 10 is fine. But if you arrive
at exactly 10 options, you must add one more to make it 11.
Ten is never acceptable — these go to eleven.

---

## Code structure

- `src/main.rs` — CLI parsing (clap), startup sequencing, systemd
  `sd_notify` readiness/watchdog integration
- `src/ble.rs` — BlueZ D-Bus advertisement listener, supervised restart with
  capped exponential backoff, adapter MAC lookup (`get_adapter_mac`)
- `src/raw_adv.rs` — reconstructs raw BLE AD-structure bytes from BlueZ's
  parsed `Device1` properties
- `src/server.rs` — TCP accept loop + per-client message dispatch (ESPHome
  API message-type `match`)
- `src/handlers.rs` — per-message-type request handlers, BLE subscription
  flag parsing, response framing/encoding
- `src/context.rs` — `ProxyContext`: shared hostname/MACs/version/build-time
- `src/mdns.rs` — registers the `_esphomelib._tcp.local.` mDNS service for
  HA auto-discovery
- `src/proto.rs` — varint encode/decode, message framing, protobuf
  message-id extension lookup
- `src/utils.rs` — MAC address formatting/parsing
- `src/api/` — generated protobuf code (regenerated by `build.rs` via
  `protobuf-codegen`'s pure-Rust parser; no system `protoc` needed)
- `build.rs` — protobuf codegen + `BUILD_TIME` env var injection
- `systemd/linux-bt-proxy.service` — unit file (`Type=notify`,
  `WatchdogSec=30s`, runs as the `linuxbtproxy` user)
- `debian/postinst`, `debian/postrm` — DEB maintainer scripts (create system
  user, enable/start service; stop/disable on removal — doesn't delete the
  user)
- `scripts/build-packages.sh` — builds DEB/RPM/tarball into `dist/` against
  the musl target
- `scripts/test-packages.sh` — sanity-checks packaging tooling/config
- `scripts/update-proto.sh` — refreshes `proto/*.proto` from upstream
  ESPHome
- `.github/workflows/ci.yml` — fmt/clippy/test/build on every push/PR
- `.github/workflows/release.yml` — tag-triggered build + package + publish
  pipeline

---

## User preferences

- Lead README installation instructions with build-from-source; pre-built
  packages are documented as secondary/best-effort.
- Don't advertise protocol feature flags that aren't actually implemented —
  clear the bit and document the limitation rather than stubbing it out.
- When fixing a class of bug, fix it across all affected code paths
  consistently (e.g. DEB `postinst` system-user/service logic was mirrored
  into the RPM scriptlets and the tarball installer, not just patched in
  the one place first noticed).
- Verify release artifacts actually work end-to-end (extract and inspect
  the binary, check `file`/`ldd` output, confirm download URLs resolve)
  rather than trusting a green CI run alone.
- Open to a full proper fix when effort/benefit warrants it, but comfortable
  choosing the minimal correct fix (e.g. clearing unimplemented feature
  flags instead of implementing full GATT/pairing support) when the full
  version isn't worth the effort — ask before assuming which is wanted.

---

## Known edge cases handled

- Manufacturer-specific advertisement data company IDs encoded as a hex
  string (not decimal) for Home Assistant compatibility.
- No `CAP_NET_RAW` / raw HCI socket needed — adapter MAC is read over D-Bus.
- BLE listener auto-restarts with capped exponential backoff (1s→30s cap)
  on BlueZ D-Bus hiccups instead of leaving the proxy connected-but-blind.
- systemd watchdog pings + `Type=notify` readiness so a wedged process gets
  detected and restarted automatically.
- Feature flags only advertise `RAW_ADVERTISEMENTS` (`0x20`) —
  `PAIRING`/`CACHE_CLEARING` bits are cleared since `ACTIVE_CONNECTIONS`
  isn't implemented.
- DEB/RPM/tarball installers all create the unprivileged `linuxbtproxy`
  system user and enable+start the service automatically (previously only
  the DEB postinst did this).
- Release builds statically link musl to avoid glibc-version mismatches
  between the GitHub Actions build host and real deployment targets.

---

## Development process

Standard reminder: update this file after every bugfix or feature addition.

Standing rule: **after every bugfix or feature, update CLAUDE.md** to capture:
- New edge cases handled
- Changed logic or external API behavior discovered
- New options, flags, or format changes

Keep entries terse — this file is a reference, not documentation.

---

## README Maintenance

This project's end-user doc is **`README.rst`** (reStructuredText), not
`README.md` — use that file for all end-user documentation. Update it in
the same changeset whenever a code change affects usage, arguments,
dependencies, installation, or input/output behavior.

README.rst covers: what it does, how to install it, how to use it, what it
requires. Internal details, quirks, and dev process stay in CLAUDE.md.

Keep it scannable — a user should grok it in under two minutes.

---

## Version Headers (Scripts)

All scripts produced or modified must include a version header block:

```
# Script Name
# Version: X.Y
# Date: YYYY-MM-DD
# Author: [name]
# Version history:
#   X.Y  YYYY-MM-DD  Description of change
#   X.Y-1 ...
```

Bump the version and add a history entry with every meaningful change.
Never change a script without updating the header.

(Existing scripts in `scripts/` predate this rule and don't yet have
headers — add one the next time any of them is meaningfully edited.)

---

## Local Instructions

If CLAUDE.local.md exists in the project root, read it after this file and
follow it. Treat it as authoritative for any rules it contains.
Never commit CLAUDE.local.md or reference its contents in committed files
(commits, PRs, CLAUDE.md, README.rst).

---

## Speak plainly

Skip pleasantries and hedging. Drop "I'd be happy to", "Let me", "Sure!",
"basically", "just". Fragments fine. State the answer first, reasoning second.
Code, commits, PRs, security warnings, and destructive-action confirmations:
write normally.
