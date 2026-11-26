"""Regress candidate validation, replay and measurement decisions."""

import hashlib
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from maintenance import workload


SCRIPTS = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class MaintenanceTests(unittest.TestCase):
    def test_untrusted_workload_does_not_reach_benchmark_runner(self):
        self.assertEqual(workload("Scope: tailscale-rs\nMetric: apk_bytes\n"), ("apk_bytes", "tailscale-rs"))
        for notes in ("Scope: tailscale-rs\nMetric: warm_build_seconds\n",
                      "Scope: feed\nMetric: unknown\n",
                      "Scope: feed\nMetric: cold_build_seconds\nMetric: other\n"):
            self.assertEqual(workload(notes), ("", ""))

    def test_comparison_holds_overlapping_timings_and_unstable_sizes(self):
        decide = load("measure-sdk").decision
        def timing(base, candidate):
            return ({"cold_build_seconds": base}, {"cold_build_seconds": candidate})
        self.assertEqual(decide("cold_build_seconds", "feed", [timing(10, 8), timing(11, 9)]),
                         "improvement observed")
        self.assertEqual(decide("cold_build_seconds", "feed", [timing(10, 9), timing(12, 11)]),
                         "no repeatable improvement")
        def size(base, candidate):
            return ({"apk_bytes": {"tailscale-rs": base}}, {"apk_bytes": {"tailscale-rs": candidate}})
        self.assertEqual(decide("apk_bytes", "tailscale-rs", [size(200, 180), size(200, 181)]), "inconclusive")

    def test_screen_only_repeats_a_pair_with_a_gain_on_the_claimed_metric(self):
        screen = load("measure-sdk").worth_repeating
        self.assertFalse(screen("cold_build_seconds", "feed", {"cold_build_seconds": 10},
                                {"cold_build_seconds": 10}))
        self.assertTrue(screen("cold_build_seconds", "feed", {"cold_build_seconds": 10},
                               {"cold_build_seconds": 9}))
        self.assertFalse(screen("apk_bytes", "tailscale-rs",
                                {"apk_bytes": {"tailscale-rs": 90, "protun-openwrt": 90}},
                                {"apk_bytes": {"tailscale-rs": 91, "protun-openwrt": 70}}))

    @unittest.skipUnless(shutil.which("bash") and os.name != "nt", "requires the Linux qualification shell")
    def test_patch_with_wrong_digest_or_base_leaves_checkout_clean(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, text=True).strip()
            git("init", "-q")
            git("config", "user.name", "CI fixture")
            git("config", "user.email", "fixture@example.com")
            text = root / "file.txt"
            text.write_text("baseline\n", encoding="utf-8")
            git("add", "file.txt")
            git("commit", "-qm", "baseline")
            head = git("rev-parse", "HEAD")
            text.write_text("candidate\n", encoding="utf-8")
            patch = root / "fixture.patch"
            patch.write_bytes(subprocess.check_output(["git", "diff"], cwd=root))
            git("restore", "file.txt")
            digest = hashlib.sha256(patch.read_bytes()).hexdigest()
            for bad_digest, bad_head in (("0" * 64, head), (digest, "0" * 40)):
                result = subprocess.run(["bash", str(SCRIPTS / "apply-update.sh"), str(patch),
                                         bad_digest, bad_head], cwd=root, capture_output=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(git("diff", "HEAD", "--", "file.txt"), "")
            subprocess.run(["bash", str(SCRIPTS / "apply-update.sh"), str(patch), digest, head],
                           cwd=root, check=True, capture_output=True)
            self.assertEqual(text.read_text(encoding="utf-8"), "candidate\n")

    def test_candidate_scope_excludes_trusted_build_and_publishing_code(self):
        allowed = load("validate-candidate").allowed
        self.assertTrue(allowed("package/tailscale-rs/files/stage-header.py", "upstream"))
        self.assertTrue(allowed("package/rust-library.mk", "maintenance"))
        self.assertFalse(allowed("package/rust-library.mk", "upstream"))
        self.assertFalse(allowed(".github/scripts/build-sdk.sh", "maintenance"))
        self.assertFalse(allowed("build/test.apk", "maintenance"))
