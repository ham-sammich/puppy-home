# Hosting a Den (puppy-relay)

A Den is a room on a `puppy-relay` server — a tiny TCP line-JSON relay
with **no accounts and no persistence**: rooms exist while members are
connected, and the room code is the shared secret.

## The easy way (in-app)

Den screen -> **Host a Den**. The app:

1. finds the `puppy-relay` binary (see resolution order below),
2. starts it on the first free port from **9220**,
3. joins you automatically, and
4. shows a `LAN-ip:port` + room code line to share.

Teammates enter that address + room code in their Join form. **Stopping
the host (or quitting the app) ends the den for everyone** — rooms are
in-memory.

Binary resolution order used by the app:

1. `puppy-relay` (`.exe` on Windows) **next to the app executable** —
   the shipped layout, and `target/debug` in dev once built.
2. Dev fallback only: `cargo run -q -p puppy-relay -- <port>` when run
   inside the repo with no built binary.

## Running a relay on a server

Build (on the server, or cross-compile and copy the binary):

```sh
cargo build --release -p puppy-relay
# binary at target/release/puppy-relay
```

Run:

```sh
puppy-relay 9220          # or PUPPY_RELAY_PORT=9220 puppy-relay
```

It listens on `0.0.0.0:<port>`. Members join with `server-ip:9220` and
any room code you agree on.

### Keep it alive (systemd example)

```ini
# /etc/systemd/system/puppy-relay.service
[Unit]
Description=Puppy Pack relay
After=network.target

[Service]
ExecStart=/usr/local/bin/puppy-relay 9220
Restart=on-failure
DynamicUser=yes
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now puppy-relay
```

### Firewall

Open the TCP port to your team only (the room code is a shared secret,
not authentication — don't expose the relay to the open internet):

```sh
sudo ufw allow from 10.0.0.0/8 to any port 9220 proto tcp
```

For internet-spanning teams, prefer a VPN/tailnet over a public port.

## Notes & limits

- One den per app instance today (single `PackClient` connection);
  joining multiple dens at once is ledgered as a future item.
- The relay has no TLS: treat it as LAN/VPN software.
