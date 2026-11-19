# Project relay: traffic limit

ADR 0044, step 3. A project relay runs iroh's own relay server
(`iroh-relay`). This keeps it inside the outgoing traffic its server
includes, using the numbers Hetzner bills from:

- past `ALERT_PCT` (50%) of the month's included traffic, a warning in the
  journal (`journalctl -t relay-traffic`);
- past `REFUSE_PCT` (90%), the relay is stopped and kept from starting until
  the count is back under, at the next billing month. Nodes turn to their
  fallback relays meanwhile, and come back by themselves.

## Setup, as root on the relay's server

1. A read-only API token for the server's Hetzner project, in
   `/etc/relay-traffic/hcloud-token` (mode 600). It never goes in the repo.
2. Install:

   ```sh
   install -m 755 traffic-check.sh /usr/local/sbin/relay-traffic-check
   install -m 644 relay-traffic.service relay-traffic.timer /etc/systemd/system/
   install -D -m 644 iroh-relay-traffic.conf /etc/systemd/system/iroh-relay.service.d/traffic.conf
   systemctl daemon-reload
   systemctl enable --now relay-traffic.timer
   relay-traffic-check status
   ```

3. Other thresholds, in `/etc/relay-traffic/limits`:

   ```sh
   ALERT_PCT=50
   REFUSE_PCT=90
   ```

   Decimals work, which is how the limit is tested: set `REFUSE_PCT` below
   the current share, run `relay-traffic-check`, check that nodes fall back,
   then remove the setting and run it again.
