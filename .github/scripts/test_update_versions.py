"""Regression tests for upstream and locally maintained package updates."""

import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("update_versions", Path(__file__).with_name("update-versions.py"))
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)


class UpdateTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for source in [ROOT / updater.WORKFLOW, *ROOT.glob("package/*/Makefile")]:
            destination = self.root / source.relative_to(ROOT)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        workflow = (self.root / updater.WORKFLOW).read_text(encoding="utf-8")
        self.sdk = updater.match_one(r"^\s+SDK_ARCH: ([a-z0-9_-]+)-" + updater.VERSION + "$", workflow)
        self.current_sdk = self.sdk.group(2)
        major, minor, micro = updater.version(self.current_sdk)
        self.next_sdk = f"{major}.{minor}.{micro + 1}"
        tags = {}
        for name, project in updater.PROJECTS.items():
            text = (self.root / "package" / name / "Makefile").read_text(encoding="utf-8")
            release = updater.match_one(r"^PKG_VERSION:=" + updater.VERSION + "$", text).group(1)
            url = updater.match_one(r"^PKG_SOURCE_URL:=(https://\S+)\?$", text).group(1)
            tags[project["repository"]] = url.rsplit("/", 1)[1].replace("$(PKG_VERSION)", release)
        self.tags = tags
        self.latest_openwrt = self.start_patch("latest_openwrt", return_value=self.current_sdk)
        self.sdk_available = self.start_patch("sdk_available", return_value=True)
        self.latest_tag = self.start_patch("latest_tag", side_effect=tags.__getitem__)
        self.updated_source = self.start_patch("updated_source", return_value=("a" * 64, b"new lock\n"))

    def start_patch(self, name, **kwargs):
        patcher = patch.object(updater, name, **kwargs)
        mock = patcher.start()
        self.addCleanup(patcher.stop)
        return mock

    def snapshot(self):
        return {p.relative_to(self.root): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}

    def test_current_releases_leave_local_and_upstream_packages_unchanged(self):
        before = self.snapshot()
        self.assertEqual(updater.update(self.root), [])
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.latest_tag.call_count, len(updater.PROJECTS))
        self.updated_source.assert_not_called()

    def test_sdk_update_bumps_every_package_including_local_package(self):
        before = self.snapshot()
        self.latest_openwrt.return_value = self.next_sdk
        updater.update(self.root)
        self.sdk_available.assert_called_once_with(self.sdk.group(1), self.next_sdk)
        for path, data in before.items():
            if path.name == "Makefile":
                text = data.decode()
                revision = int(updater.match_one(r"^PKG_RELEASE:=(\d+)$", text).group(1))
                expected = text.replace(f"PKG_RELEASE:={revision}\n", f"PKG_RELEASE:={revision + 1}\n")
                self.assertEqual((self.root / path).read_text(encoding="utf-8"), expected)
        self.assertIn(f"SDK_ARCH: {self.sdk.group(1)}-{self.next_sdk}",
                      (self.root / updater.WORKFLOW).read_text(encoding="utf-8"))
        self.updated_source.assert_not_called()

    def test_unpublished_sdk_leaves_all_packages_unchanged(self):
        before = self.snapshot()
        self.latest_openwrt.return_value = self.next_sdk
        self.sdk_available.return_value = False
        self.assertEqual(updater.update(self.root), [])
        self.assertEqual(self.snapshot(), before)

    def test_unknown_package_is_rejected_before_network_access(self):
        recipe = self.root / "package/unexpected/Makefile"
        recipe.parent.mkdir()
        recipe.write_text("PKG_NAME:=unexpected\n", encoding="utf-8")
        before = self.snapshot()
        with self.assertRaisesRegex(ValueError, r"unconfigured recipes=\['unexpected'\]"):
            updater.update(self.root)
        self.latest_openwrt.assert_not_called()
        self.assertEqual(self.snapshot(), before)

    def test_missing_package_is_rejected_before_network_access(self):
        for name in [*updater.PROJECTS, *updater.LOCAL_PACKAGES]:
            with self.subTest(package=name):
                recipe = self.root / "package" / name / "Makefile"
                data = recipe.read_bytes()
                recipe.unlink()
                try:
                    with self.assertRaisesRegex(ValueError, f"missing recipes=\\['{name}'\\]"):
                        updater.update(self.root)
                    self.latest_openwrt.assert_not_called()
                finally:
                    recipe.write_bytes(data)

    def test_upstream_release_updates_without_changing_local_package(self):
        name = "protun-openwrt"
        project = updater.PROJECTS[name]
        current = self.tags[project["repository"]]
        major, minor, micro = updater.version(current.removeprefix("v"))
        release = f"{major}.{minor}.{micro + 1}"
        self.tags[project["repository"]] = ("v" if current.startswith("v") else "") + release
        local_path = self.root / "package/proton-account/Makefile"
        local_before = local_path.read_bytes()
        (self.root / "package" / name / "files").mkdir()
        updater.update(self.root)
        text = (self.root / "package" / name / "Makefile").read_text(encoding="utf-8")
        self.assertIn(f"PKG_VERSION:={release}\n", text)
        self.assertIn("PKG_RELEASE:=1\n", text)
        self.assertIn("PKG_HASH:=" + "a" * 64 + "\n", text)
        self.assertEqual((self.root / "package" / name / "files/Cargo.lock").read_bytes(), b"new lock\n")
        self.assertEqual(local_path.read_bytes(), local_before)
        self.updated_source.assert_called_once()


if __name__ == "__main__":
    unittest.main()
