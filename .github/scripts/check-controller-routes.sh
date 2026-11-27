#!/usr/bin/env bash
# Run only in a disposable network namespace. WAN is a fixture veth pair.
set -euo pipefail
host_namespace=${1:?pass the original network namespace identity}
current_namespace=$(readlink /proc/self/ns/net)
test -n "$host_namespace" && test "$host_namespace" != "$current_namespace"

ip link add pn-wan type veth peer name pn-receiver
for device in pn-wan pn-receiver; do
    ip link set "$device" addrgenmode none up
done
receiver_mac=$(cat /sys/class/net/pn-receiver/address)
ip address add 192.0.2.1/24 dev pn-wan
ip -6 address add 2001:db8::1/64 dev pn-wan nodad
ip neigh replace 192.0.2.2 lladdr "$receiver_mac" nud permanent dev pn-wan
ip -6 neigh replace 2001:db8::2 lladdr "$receiver_mac" nud permanent dev pn-wan

PROTON_CONTROLLER_BACKEND=protun
uci() { echo wireguard-udp; }
ensure_exit_rule() {
    local family=$1 priority=$2 line=$3
    shift 3
    if ! ip "-$family" rule show | grep -Fxq "$line"; then
        ip "-$family" rule add pref "$priority" "$@"
    fi
}
. package/protun-openwrt/files/controller/backend.sh
for family in 4 6; do
    ip "-$family" rule add pref 10600 fwmark 0x620 lookup 620
done
ip link add pt-proton type dummy
ip link set pt-proton up
vpn_reconcile true
for family in 4 6; do
    ip "-$family" route show table 620 | grep -F 'default dev pt-proton'
done
ip link del pt-proton
vpn_reconcile true

python3 - <<'PY'
import errno
import socket
import time
from pathlib import Path

counter = Path("/sys/class/net/pn-receiver/statistics/rx_packets")
destinations = ((socket.AF_INET, "192.0.2.2"), (socket.AF_INET6, "2001:db8::2"))

def probe(mark):
    for family, host in destinations:
        with socket.socket(family, socket.SOCK_DGRAM) as sender:
            if mark:
                sender.setsockopt(socket.SOL_SOCKET, socket.SO_MARK, mark)
            try:
                sender.sendto(b"synthetic controller route test", (host, 9420))
            except OSError as error:
                if not mark or error.errno not in (errno.ENETUNREACH, errno.EHOSTUNREACH):
                    raise
            else:
                if mark:
                    raise AssertionError("VPN-selected traffic escaped the unreachable route")

def received():
    return int(counter.read_text())

before = received()
probe(0)
deadline = time.monotonic() + 2
while received() < before + len(destinations):
    if time.monotonic() >= deadline:
        raise AssertionError("physical receiver did not see the unmarked control traffic")
    time.sleep(0.01)
before = received()
probe(0x620)
assert received() == before, "VPN-selected traffic reached the physical receiver"
print("Both families stayed fail-closed after tunnel device loss")
PY
