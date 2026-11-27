"""Exercise the installed controller command seam with a disposable host fixture."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


CONTROLLER = Path(__file__).resolve().parents[2] / "package/protun-openwrt/files/controller"

COMMON = """
PROTON_RUNTIME=$FIXTURE/run
PROTON_CONTROLLER_BACKEND=$BACKEND
uci() { echo "$PROTOCOL"; }
uplink_device() { echo wan; }
unsigned_integer() { case "$1" in ''|*[!0-9]*) return 1;; esac; }
monotonic_seconds() { echo 100; }
try_lock() { flock -n "$1"; }
wait_lock() { flock -w "$2" "$1"; }
ensure_exit_rule() {
    local family="$1" priority="$2"
    shift 3
    ip "-$family" rule show | grep -Fq "$*" ||
        ip "-$family" rule add pref "$priority" "$@"
}
ip() {
    printf '%s\n' "$*" >> "$FIXTURE/ip.log"
    case "$*" in
        *'link show dev pt-proton') test -e "$FIXTURE/sys/pt-proton" || return 1
            echo '3: pt-proton: <UP,LOWER_UP>'
            ;;
        '-o address show dev pt-proton') echo '10.2.0.2/32';;
        '-6 -o address show dev pt-proton') echo '2a07:b944::2:2/128';;
        *' rule show pref 10900 oif pt-proton lookup 620')
            sed -n '/^10900:.*oif pt-proton lookup 620$/p' "$FIXTURE/rules-${1#-}";;
        *' rule show') cat "$FIXTURE/rules-${1#-}";;
        *' rule del pref 10900') sed -i '0,/^10900:/{/^10900:/d;}' "$FIXTURE/rules-${1#-}";;
        *' rule del pref 10900 oif pt-proton lookup 620')
            sed -i '/^10900:.*oif pt-proton lookup 620$/d' "$FIXTURE/rules-${1#-}";;
        *' rule add pref 10900 oif pt-proton lookup 620')
            echo '10900: from all oif pt-proton lookup 620' >> "$FIXTURE/rules-${1#-}";;
    esac
    return 0
}
jsonfilter() {
    case "$1" in
        -i) jq -er "${4#@}" "$2";;
        -s) printf '%s' "$2" | jq -er "${4#@}";;
    esac
}
ubus() { echo '{}'; }
controller_service() {
    case "$1" in
        start) touch "$FIXTURE/sys/pt-proton";;
        stop) rm -f "$FIXTURE/sys/pt-proton";;
    esac
}
controller_client() { test "$1" = --check-config && jq -e . "$2" >/dev/null; }
curl() {
    printf '%s\n' "$1" >> "$FIXTURE/probes"
    if [ "${REPLACE_WORKER:-}" = "$1" ]; then
        jq --argjson pid "$REPLACEMENT_PID" '.pid = $pid' "$PROTON_RUNTIME/stealth-state.json" > "$FIXTURE/next"
        mv "$FIXTURE/next" "$PROTON_RUNTIME/stealth-state.json"
    fi
    if [ "${STOP_DURING_PROBE:-}" = "$1" ]; then
        "$FIXTURE/proton-stealth" stop || return 1
    fi
    [ "${FAIL_PROBE:-}" != "$1" ] || return 1
    echo 'ip=192.0.2.20'
}
. "$FIXTURE/backend.sh"
"""

# Only the fields consumed by the isolated host are needed for the fixture.
JSHN = """
json_init() { fixture_json='{}'; }
json_add_string() { fixture_json=$(printf '%s' "$fixture_json" | jq -c --arg k "$1" --arg v "$2" '.[$k] = $v'); }
json_add_int() { [ -z "$1" ] || fixture_json=$(printf '%s' "$fixture_json" | jq -c --arg k "$1" --argjson v "$2" '.[$k] = $v'); }
json_add_array() { :; }
json_close_array() { :; }
json_add_boolean() { :; }
json_dump() { printf '%s\n' "$fixture_json"; }
"""


@unittest.skipUnless(sys.platform == "linux" and shutil.which("jq"), "requires Linux and jq")
class ControllerContract(unittest.TestCase):
    def setUp(self):
        fixture = tempfile.TemporaryDirectory()
        self.addCleanup(fixture.cleanup)
        self.root = Path(fixture.name)
        self.run_dir = self.root / "run"
        for folder in ("run", "sys", "private", "processes"):
            (self.root / folder).mkdir()
        self.env = {**os.environ, "FIXTURE": str(self.root), "BACKEND": "protun",
                    "PROTOCOL": "wireguard-udp", "REPLACEMENT_PID": str(os.getpid())}
        self.write("common.sh", COMMON)
        self.write("jshn.sh", JSHN)
        self.write("processes/boot_id", "fixture-boot\n")
        self.write("processes/uuid", "fixture-instance\n")
        self.write("private/active.conf",
                   "Endpoint = 192.0.2.10:51820\nPrivateKey = synthetic\n"
                   "PublicKey = synthetic\nAddress = 10.2.0.2/32, 2a07:b944::2:2/128\n")
        for family in (4, 6):
            self.write(f"rules-{family}", "")
        for name in ("backend.sh", "proton-stealth", "proton-stealth.init"):
            source = (CONTROLLER / name).read_text()
            for original, fake in {
                "/usr/lib/exit-node-common": '"$FIXTURE/common.sh"',
                "/usr/share/libubox/jshn.sh": '"$FIXTURE/jshn.sh"',
                "/root/proton-private": '"$FIXTURE/private"',
                "/sys/class/net": "$FIXTURE/sys",
                "/proc/sys/kernel/random": "$FIXTURE/processes",
                "/proc/": "$FIXTURE/processes/",
                "/etc/init.d/proton-stealth": "controller_service",
                "/usr/sbin/protun-openwrt": "controller_client",
            }.items():
                source = source.replace(original, fake)
            path = self.write(name, source)
            path.chmod(0o700)

    def write(self, name, value):
        path = self.root / name
        path.write_text(value)
        return path

    def ready_worker(self, **changes):
        pid = os.getppid()
        process = self.root / f"processes/{pid}"
        process.mkdir(exist_ok=True)
        self.write(f"processes/{pid}/comm", "protun-openwrt\n")
        self.write("run/stealth-wanted", "")
        self.write("run/stealth.json", '{"status_instance":"fixture-instance"}')
        status = {"ready": True, "protocol": "wireguard-udp", "monotonic": 100,
                  "boot_id": "fixture-boot", "instance": "fixture-instance", "pid": pid}
        self.write("run/stealth-state.json", json.dumps(status | changes))
        self.write("sys/pt-proton", "")

    def run_command(self, *command, **env):
        return subprocess.run(command, cwd=self.root, env=self.env | env,
                              capture_output=True, text=True, timeout=15)

    def invoke(self, command, **env):
        return self.run_command("sh", str(self.root / "proton-stealth"), command, **env)

    def test_direct_boot_leaves_service_stopped(self):
        result = self.run_command("sh", "-ec", '. "$FIXTURE/proton-stealth.init"; boot')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.run_dir / "stealth-wanted").exists())
        self.assertFalse((self.root / "sys/pt-proton").exists())

    def test_prepare_restores_addressing_and_routes_for_both_families(self):
        result = self.invoke("prepare")
        self.assertEqual(result.returncode, 0, result.stderr)
        routes = (self.root / "ip.log").read_text()
        for family in (4, 6):
            self.assertIn(f"-{family} route replace default dev pt-proton table 620", routes)
            self.assertIn(f"-{family} route replace unreachable default table 620", routes)

    def test_prepare_and_repeated_health_preserve_unrelated_rules(self):
        for family in (4, 6):
            self.write(f"rules-{family}", "10900: from all lookup 901\n")

        result = self.invoke("prepare")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.ready_worker()
        for _ in range(2):
            result = self.invoke("health")
            self.assertEqual(result.returncode, 0, result.stderr)
        for family in (4, 6):
            rules = (self.root / f"rules-{family}").read_text().splitlines()
            self.assertCountEqual(rules, ["10900: from all lookup 901",
                                          "10900: from all oif pt-proton lookup 620"])

        self.assertEqual(self.invoke("stop").returncode, 0)
        for family in (4, 6):
            self.assertEqual((self.root / f"rules-{family}").read_text(),
                             "10900: from all lookup 901\n")

    def test_health_repairs_tun_addressing_and_requires_both_probes(self):
        self.ready_worker()
        result = self.invoke("health")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "probes").read_text().splitlines(), ["-4", "-6"])
        routes = (self.root / "ip.log").read_text()
        self.assertIn("address replace 10.2.0.2/32 dev pt-proton", routes)
        self.assertIn("-6 address replace 2a07:b944::2:2/128 dev pt-proton", routes)
        for family in ("-4", "-6"):
            with self.subTest(family=family):
                result = self.invoke("health", FAIL_PROBE=family)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((self.run_dir / "stealth-health.json").exists())
                self.assertEqual(self.invoke("health").returncode, 0)

    def test_missing_stale_or_wrong_instance_never_confirms_readiness(self):
        for changes in ({"monotonic": 96}, {"instance": "old"}, {"boot_id": "old"},
                        {"protocol": "stealth"}, {"ready": False}):
            with self.subTest(changes=changes):
                self.ready_worker(**changes)
                self.assertNotEqual(self.invoke("health").returncode, 0)
                self.assertFalse((self.run_dir / "stealth-health.json").exists())
        self.ready_worker()
        (self.run_dir / "stealth-state.json").unlink()
        self.assertNotEqual(self.invoke("health").returncode, 0)

    def test_old_probe_cannot_confirm_replacement_worker(self):
        self.ready_worker()
        process = self.root / f"processes/{os.getpid()}"
        process.mkdir(exist_ok=True)
        self.write(f"processes/{os.getpid()}/comm", "protun-openwrt\n")
        result = self.invoke("health", REPLACE_WORKER="-6")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.run_dir / "stealth-health.json").exists())

    def test_stop_during_probe_does_not_republish_readiness(self):
        self.ready_worker()
        result = self.invoke("health", STOP_DURING_PROBE="-6")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.run_dir / "stealth-wanted").exists())
        self.assertFalse((self.run_dir / "stealth-health.json").exists())
