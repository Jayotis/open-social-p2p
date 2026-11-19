#!/bin/bash
# Keeps a project relay inside the traffic its hosting includes (ADR 0044,
# step 3). Hetzner bills outgoing traffic above what the server includes, so
# this reads both numbers from Hetzner's API, as billed, and:
#
#   - logs a warning once the month's outgoing traffic passes ALERT_PCT;
#   - stops the relay at REFUSE_PCT, and keeps it from starting again (a
#     reboot included) until the count drops back, at the next billing month.
#
# Nodes then turn to their fallback relays (`relay_fallback`), and come back
# once the relay lets them in again.
#
# Setup on the server: see scripts/relay/README.md.
#
#   traffic-check.sh          check once (the timer runs this)
#   traffic-check.sh status   print the numbers and state, change nothing
set -euo pipefail

CONF=/etc/relay-traffic
STATE=/var/lib/relay-traffic
REFUSED=$STATE/refused     # present while the relay is held off
ALERTED=$STATE/alerted     # present once this month's warning was logged
SERVICE=${SERVICE:-iroh-relay}
ALERT_PCT=50
REFUSE_PCT=90
# shellcheck source=/dev/null
[ -f "$CONF/limits" ] && . "$CONF/limits"

log() { logger -t relay-traffic -p "user.$1" -- "$2"; echo "$2"; }

token=$(cat "$CONF/hcloud-token")
ipv4=$(ip -4 -o addr show scope global | awk '{split($4, a, "/"); print a[1]; exit}')
json=$(curl -fsS --max-time 30 -H "Authorization: Bearer $token" \
  "https://api.hetzner.cloud/v1/servers?per_page=50")

# Prints: outgoing included percent (of included), for the server with our IP.
read -r out inc pct < <(IPV4=$ipv4 python3 -I -c '
import json, os, sys
servers = json.load(sys.stdin)["servers"]
me = [s for s in servers if (s["public_net"].get("ipv4") or {}).get("ip") == os.environ["IPV4"]]
if len(me) != 1:
    sys.exit("no server with this address in the API answer")
out, inc = me[0]["outgoing_traffic"] or 0, me[0]["included_traffic"]
print(out, inc, f"{100 * out / inc:.4f}")
' <<<"$json")

over() { python3 -I -c "import sys; sys.exit(0 if $pct >= $1 else 1)"; }
gb() { python3 -I -c "print(f'{$1 / 1e9:.1f} GB')"; }
summary="outgoing $(gb "$out") of $(gb "$inc") included ($pct%)"

if [ "${1:-}" = status ]; then
  echo "$summary; alert at $ALERT_PCT%, refuse at $REFUSE_PCT%"
  echo "relay: $(systemctl is-active "$SERVICE" || true)$([ -f "$REFUSED" ] && echo ', held off by the traffic limit')"
  exit 0
fi

mkdir -p "$STATE"
if over "$REFUSE_PCT"; then
  if [ ! -f "$REFUSED" ]; then
    touch "$REFUSED"
    log crit "refusing relay traffic until next month: $summary"
  fi
  systemctl stop "$SERVICE"
elif [ -f "$REFUSED" ]; then
  # Back under the limit: a new billing month, or a raised limit.
  rm -f "$REFUSED" "$ALERTED"
  log notice "relay traffic allowed again: $summary"
  systemctl start "$SERVICE"
fi
if over "$ALERT_PCT"; then
  if [ ! -f "$ALERTED" ]; then
    touch "$ALERTED"
    log warning "relay traffic past $ALERT_PCT%: $summary"
  fi
else
  rm -f "$ALERTED"
fi
