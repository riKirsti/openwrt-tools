# Transport context is separate from the compatible per-network VPN toggle.
vpn_context() {
 VPN_BACKEND="$(uci -q get exitnode.main.proton_backend || echo wireguard)"
 case "$VPN_BACKEND" in
  wireguard) VPN_DEV=wg-proton; VPN_PROTOCOL=wireguard-udp;;
  stealth) VPN_DEV=pt-proton; VPN_PROTOCOL=stealth;;
  *) return 2;;
 esac
}
vpn_engine_live() {
 local latest now pid boot instance configured
 if [ "$VPN_BACKEND" = wireguard ]; then
  ip -o link show dev wg-proton 2>/dev/null | grep -Eq '<([^>]*,)?UP(,|>)' || return 1
  latest="$(wg show wg-proton latest-handshakes 2>/dev/null | awk '$2 > latest {latest=$2} END {print latest+0}')"
  now="$(date +%s)"; [ "$latest" -gt 0 ] && [ "$latest" -le "$now" ] && [ $((now-latest)) -le 180 ]; return
 fi
 [ -f "$PROTON_RUNTIME/stealth-wanted" ] && [ -s "$PROTON_RUNTIME/stealth-state.json" ] || return 1
 [ "$(jsonfilter -i "$PROTON_RUNTIME/stealth-state.json" -e '@.ready' 2>/dev/null)" = true ] || return 1
 [ "$(jsonfilter -i "$PROTON_RUNTIME/stealth-state.json" -e '@.protocol')" = stealth ] || return 1
 latest="$(jsonfilter -i "$PROTON_RUNTIME/stealth-state.json" -e '@.monotonic')"
 unsigned_integer "$latest" || return 1
 now="$(monotonic_seconds)"; [ "$latest" -le "$now" ] && [ $((now-latest)) -le 3 ] || return 1
 boot="$(jsonfilter -i "$PROTON_RUNTIME/stealth-state.json" -e '@.boot_id')"
 [ "$boot" = "$(cat /proc/sys/kernel/random/boot_id)" ] || return 1
 instance="$(jsonfilter -i "$PROTON_RUNTIME/stealth-state.json" -e '@.instance')"
 configured="$(jsonfilter -i "$PROTON_RUNTIME/stealth.json" -e '@.status_instance')"
 [ -n "$instance" ] && [ "$instance" = "$configured" ] || return 1
 pid="$(jsonfilter -i "$PROTON_RUNTIME/stealth-state.json" -e '@.pid')"
 unsigned_integer "$pid" && kill -0 "$pid" 2>/dev/null || return 1
 [ "$(cat /proc/"$pid"/comm 2>/dev/null)" = protun-openwrt ] || return 1
}
vpn_engine_ready() {
 vpn_engine_live || return 1
 ip -o link show dev "$VPN_DEV" 2>/dev/null | grep -Eq '<([^>]*,)?UP(,|>)'
}
vpn_ready() {
 local checked now pid health_pid instance
 vpn_engine_ready || return 1
 [ "$VPN_BACKEND" = stealth ] || return 0
 [ -s "$PROTON_RUNTIME/stealth-health.json" ] || return 1
 checked="$(jsonfilter -i "$PROTON_RUNTIME/stealth-health.json" -e '@.monotonic')"
 unsigned_integer "$checked" || return 1
 now="$(monotonic_seconds)"; [ "$checked" -le "$now" ] && [ $((now-checked)) -le 65 ] || return 1
 pid="$(jsonfilter -i "$PROTON_RUNTIME/stealth-state.json" -e '@.pid')"
 health_pid="$(jsonfilter -i "$PROTON_RUNTIME/stealth-health.json" -e '@.pid')"
 [ "$pid" = "$health_pid" ] || return 1
 instance="$(jsonfilter -i "$PROTON_RUNTIME/stealth.json" -e '@.status_instance')"
 [ "$instance" = "$(jsonfilter -i "$PROTON_RUNTIME/stealth-health.json" -e '@.instance')" ] || return 1
}
vpn_reconcile() {
 local family
 [ "${1:-false}" = true ] && [ "$VPN_BACKEND" = stealth ] || return 0
 for family in 4 6; do
  ip -"$family" route replace unreachable default table 620 metric 32000
  if ip link show dev "$VPN_DEV" >/dev/null 2>&1; then
   ip -"$family" route replace default dev "$VPN_DEV" table 620 metric 10
   ensure_exit_rule "$family" 10900 "10900: from all oif $VPN_DEV lookup 620" oif "$VPN_DEV" lookup 620
  fi
 done
}
vpn_context
