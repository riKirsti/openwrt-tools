# Transport context is separate from the compatible per-network VPN toggle.
vpn_context() {
 VPN_PROTOCOL="$(uci -q get exitnode.main.proton_protocol || true)"
 if [ -z "$VPN_PROTOCOL" ]; then
  case "$(uci -q get exitnode.main.proton_backend || echo stealth)" in
   stealth|protun) VPN_PROTOCOL=stealth;;
   wireguard) VPN_PROTOCOL=wireguard-udp;;
   wireguard-tcp) VPN_PROTOCOL=wireguard-tcp;;
   *) return 2;;
  esac
 fi
 case "$VPN_PROTOCOL" in stealth|wireguard-udp|wireguard-tcp) ;; *) return 2;; esac
 VPN_BACKEND=protun
 VPN_DEV=pt-proton
}
vpn_engine_live() {
 local latest now pid boot instance configured
 [ -f "$PROTON_RUNTIME/client-wanted" ] && [ -s "$PROTON_RUNTIME/client-state.json" ] || return 1
 [ "$(jsonfilter -i "$PROTON_RUNTIME/client-state.json" -e '@.ready' 2>/dev/null)" = true ] || return 1
 [ "$(jsonfilter -i "$PROTON_RUNTIME/client-state.json" -e '@.protocol')" = "$VPN_PROTOCOL" ] || return 1
 latest="$(jsonfilter -i "$PROTON_RUNTIME/client-state.json" -e '@.monotonic')"
 unsigned_integer "$latest" || return 1
 now="$(monotonic_seconds)"; [ "$latest" -le "$now" ] && [ $((now-latest)) -le 3 ] || return 1
 boot="$(jsonfilter -i "$PROTON_RUNTIME/client-state.json" -e '@.boot_id')"
 [ "$boot" = "$(cat /proc/sys/kernel/random/boot_id)" ] || return 1
 instance="$(jsonfilter -i "$PROTON_RUNTIME/client-state.json" -e '@.instance')"
 configured="$(jsonfilter -i "$PROTON_RUNTIME/client.json" -e '@.status_instance')"
 [ -n "$instance" ] && [ "$instance" = "$configured" ] || return 1
 pid="$(jsonfilter -i "$PROTON_RUNTIME/client-state.json" -e '@.pid')"
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
 [ -s "$PROTON_RUNTIME/client-health.json" ] || return 1
 checked="$(jsonfilter -i "$PROTON_RUNTIME/client-health.json" -e '@.monotonic')"
 unsigned_integer "$checked" || return 1
 now="$(monotonic_seconds)"; [ "$checked" -le "$now" ] && [ $((now-checked)) -le 65 ] || return 1
 pid="$(jsonfilter -i "$PROTON_RUNTIME/client-state.json" -e '@.pid')"
 health_pid="$(jsonfilter -i "$PROTON_RUNTIME/client-health.json" -e '@.pid')"
 [ "$pid" = "$health_pid" ] || return 1
 instance="$(jsonfilter -i "$PROTON_RUNTIME/client.json" -e '@.status_instance')"
 [ "$instance" = "$(jsonfilter -i "$PROTON_RUNTIME/client-health.json" -e '@.instance')" ] || return 1
}
vpn_reconcile() {
 local family
 [ "${1:-false}" = true ] || return 0
 for family in 4 6; do
  ip -"$family" route replace unreachable default table 620 metric 32000
  if ip link show dev "$VPN_DEV" >/dev/null 2>&1; then
   ip -"$family" route replace default dev "$VPN_DEV" table 620 metric 10
   ensure_exit_rule "$family" 10900 "10900: from all oif $VPN_DEV lookup 620" oif "$VPN_DEV" lookup 620
  fi
 done
}
vpn_context
