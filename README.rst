Linux Bluetooth Proxy for ESPHome
=================================

This project provides a Bluetooth proxy daemon for ESPHome, designed to run on Linux systems. It listens for Bluetooth Low Energy (BLE) advertisements using the BlueZ stack and forwards them over TCP to ESPHome or other compatible clients (such as Home Assistant). The proxy also advertises itself via mDNS as esphomelib for easy network discovery.

This is a fork of `reedstrm/linux_bt_proxy <https://github.com/reedstrm/linux_bt_proxy>`_. Compared to upstream, this fork:

- Implements real raw BLE advertisement forwarding (``BluetoothLERawAdvertisementsResponse``) instead of advertising the feature without sending any data for it
- Fixes manufacturer-specific advertisement data being silently corrupted (company IDs were encoded as decimal instead of the hex string Home Assistant expects)
- No longer needs ``CAP_NET_RAW`` or a raw HCI socket — the Bluetooth adapter's address is read over D-Bus instead
- Restarts the BLE listener with backoff instead of leaving the proxy connected-but-blind after a BlueZ hiccup
- Integrates with systemd (``sd_notify`` readiness and watchdog pings) so a wedged process is detected and restarted automatically
- Builds with a pure-Rust protobuf toolchain, so a system ``protoc`` binary is no longer required

It uses the BlueZ stack via D-Bus, so it cooperates with desktop and other system usage of the Bluetooth hardware rather than taking exclusive control of it.

Requirements
------------

- Linux with BlueZ (``bluetoothd``) and a D-Bus system bus — both are present by default on any systemd-based distribution with Bluetooth support installed
- A Bluetooth adapter that BlueZ has registered (check with ``bluetoothctl list``)
- systemd, if using the provided service unit and packages (not required to just run the binary directly)

No special capabilities or root privileges are required at runtime — the daemon talks to BlueZ entirely over D-Bus and runs as an unprivileged system user.

Installation
------------

**Debian/Ubuntu (DEB packages)**

System packages for Debian-based systems (Debian, Ubuntu, Pop-OS) are provided as part of the release package:

.. code-block:: bash

   sudo dpkg -i linux-bt-proxy_*.deb

**Red Hat/Fedora/CentOS (RPM packages)**

RPM packages are available for Red Hat-based systems:

.. code-block:: bash

   sudo rpm -i linux-bt-proxy-*.rpm
   # or with dnf/yum:
   sudo dnf install linux-bt-proxy-*.rpm

**Arch Linux (Tarball)**

For Arch Linux and other distributions, extract the tarball and run the install script:

.. code-block:: bash

   tar -xzf linux-bt-proxy-*-x86_64-unknown-linux-gnu.tar.gz
   cd linux-bt-proxy-*
   sudo ./install.sh

All three package formats install the binary and systemd unit, create an unprivileged ``linuxbtproxy`` system user (a member of the ``bluetooth`` group via the unit's ``SupplementaryGroups=``), and enable and start the service automatically. There's nothing further to run after installation — check that it came up with:

.. code-block:: bash

   sudo systemctl status linux-bt-proxy

To remove a tarball install, run ``sudo ./uninstall.sh`` from the extracted directory; DEB/RPM removal is handled by ``dpkg -r``/``rpm -e`` as usual.

Configuration
--------------

The daemon is configured entirely through command-line flags; there is no config file. When installed via package, it runs with no flags (adapter ``hci0``, listening on ``[::]:6053``). To customize, override the unit's ``ExecStart``:

.. code-block:: bash

   sudo systemctl edit linux-bt-proxy

and add:

.. code-block:: ini

   [Service]
   ExecStart=
   ExecStart=/usr/bin/linux_bt_proxy --hci 1 --hostname freezer-bt-proxy

then ``sudo systemctl restart linux-bt-proxy``.

Options:

- ``-a, --hci <INDEX>``: Bluetooth adapter index (default: 0 for hci0)
- ``-l, --listen <ADDR>``: TCP listen address — binds dual-stack IPv4+IPv6 by default (default: ``[::]:6053``)
- ``--hostname <NAME>``: Hostname to advertise (default: system hostname)
- ``-m, --mac <MAC>``: MAC address for mDNS (optional; auto-detected if omitted)

Usage
-----

For testing and development, you may run the proxy daemon directly with:

.. code-block:: bash

   cargo run --release -- [OPTIONS]

Example:

.. code-block:: bash

   cargo run --release -- --hci 1 --listen 192.168.1.10:6053 --hostname my-bt-proxy

Verifying it's running
-----------------------

.. code-block:: bash

   sudo systemctl status linux-bt-proxy
   journalctl -u linux-bt-proxy -f

A healthy startup looks like:

.. code-block::

   Listening for ble advertisements on hci0
   mDNS service registered
   Listening on [::]:6053

Once running, it should appear in Home Assistant under *Settings → Devices & Services* as a discovered Bluetooth proxy (via mDNS/zeroconf) — no manual configuration is needed on the Home Assistant side.

If the BLE listener loses its BlueZ connection, it logs a warning and retries with exponential backoff (1s, doubling up to a 30s cap) rather than killing the proxy; ``journalctl`` will show these retries. If the whole process wedges, systemd's watchdog (``WatchdogSec=30s`` in the unit) will detect the missed heartbeat and restart it.

Building
--------

Requires a Rust toolchain (edition 2021 or newer, e.g. via `rustup <https://rustup.rs>`_) and a Linux system with BlueZ. No system ``protoc`` is needed — protobuf code is generated with a pure-Rust parser at build time.

.. code-block:: bash

   cargo build --release

Packaging
---------

To build all package formats (DEB, RPM, and tarball):

.. code-block:: bash

   ./scripts/build-packages.sh

This will create packages in the ``dist/`` directory:

- ``*.deb`` - Debian/Ubuntu packages
- ``*.rpm`` - Red Hat/Fedora/CentOS packages
- ``*.tar.gz`` - Generic tarball for Arch Linux and other distributions

Prerequisites for packaging:

.. code-block:: bash

   cargo install cargo-deb cargo-generate-rpm

Releasing
---------

Releases are automatically built and published when version tags are pushed:

.. code-block:: bash

   # Update version in Cargo.toml first, then:
   git tag v0.1.1
   git push origin v0.1.1

This triggers a GitHub Actions workflow that:

- Builds DEB, RPM, and tarball packages
- Creates a GitHub release with auto-generated notes
- Uploads all package formats as release assets

The workflow validates that the tag version matches ``Cargo.toml`` before building.

Project Structure
-----------------

- ``src/main.rs``: Entry point and CLI handling
- ``src/ble.rs``: BLE advertisement listener logic (BlueZ D-Bus, supervised restart)
- ``src/raw_adv.rs``: Raw BLE advertising-data (AD structure) reconstruction
- ``src/mdns.rs``: mDNS service registration
- ``src/server.rs``: TCP server implementation
- ``src/context.rs``: Shared proxy context
- ``src/utils.rs``: Utility functions

Known Limitations
------------------

The proxy advertises the ``PAIRING`` and ``CACHE_CLEARING`` Bluetooth proxy features, but only advertisement forwarding (legacy and raw) is implemented; per-device GATT connections, pairing, and cache-clearing requests are not handled. This is inherited from upstream and unrelated to advertisement-only use cases (e.g. BLE sensor/thermometer monitoring), but if your setup relies on Home Assistant connecting directly to a BLE peripheral through this proxy, that path isn't implemented yet.

License
-------

This project is licensed under the GPL 3.0 or later.

Contributing
------------

Pull requests and issues are welcome! Please open an issue for bug reports or feature requests.
