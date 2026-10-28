# Kernel WireGuard transport for compatible exit-node controllers.
vpn_context() {
 VPN_BACKEND="$(uci -q get exitnode.main.proton_backend || echo wireguard)"
 [ "$VPN_BACKEND" = wireguard ] || return 2
 VPN_DEV=wg-proton; VPN_PROTOCOL=wireguard-udp
}
vpn_engine_live() {
 local latest now
 ip -o link show dev wg-proton 2>/dev/null | grep -Eq '<([^>]*,)?UP(,|>)' || return 1
 latest="$(wg show wg-proton latest-handshakes 2>/dev/null | awk '$2 > latest {latest=$2} END {print latest+0}')"
 now="$(date +%s)"
 [ "$latest" -gt 0 ] && [ "$latest" -le "$now" ] && [ $((now-latest)) -le 180 ]
}
vpn_engine_ready() {
 vpn_engine_live || return 1
 ip -o link show dev "$VPN_DEV" 2>/dev/null | grep -Eq '<([^>]*,)?UP(,|>)'
}
vpn_ready() { vpn_engine_ready; }
vpn_reconcile() { return 0; }
vpn_context
