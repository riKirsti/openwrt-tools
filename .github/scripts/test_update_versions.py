"""Regression tests for upstream package updates."""

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
        self.commits = {}
        for name, project in updater.PROJECTS.items():
            text = (self.root / "package" / name / "Makefile").read_text(encoding="utf-8")
            release = updater.match_one(r"^PKG_VERSION:=" + updater.VERSION + "$", text).group(1)
            tags[project["repository"]] = f"v{release}"
            self.commits[project["repository"]] = updater.match_one(
                r"^PKG_SOURCE_COMMIT:=(" + updater.COMMIT + ")$", text).group(1)
        self.tags = tags
        self.latest_openwrt = self.start_patch("latest_openwrt", return_value=self.current_sdk)
        self.sdk_available = self.start_patch("sdk_available", return_value=True)
        self.latest_tag = self.start_patch("latest_tag", side_effect=tags.__getitem__)
        self.release_commit = self.start_patch("release_commit", side_effect=lambda repo, tag: self.commits[repo])
        self.updated_source = self.start_patch("updated_source", return_value=("a" * 64, b"new lock\n", {}))

    def start_patch(self, name, **kwargs):
        patcher = patch.object(updater, name, **kwargs)
        mock = patcher.start()
        self.addCleanup(patcher.stop)
        return mock

    def snapshot(self):
        return {p.relative_to(self.root): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}

    def test_current_releases_leave_packages_unchanged(self):
        before = self.snapshot()
        self.assertEqual(updater.update(self.root), [])
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.latest_tag.call_count, len(updater.PROJECTS))
        self.updated_source.assert_not_called()

    def test_sdk_update_bumps_every_package(self):
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
        for name in updater.PROJECTS:
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

    def test_protunnext_update_leaves_tailscale_unchanged(self):
        name = "protun-openwrt"
        project = updater.PROJECTS[name]
        current = self.tags[project["repository"]]
        major, minor, micro = updater.version(current.removeprefix("v"))
        release = f"{major}.{minor}.{micro + 1}"
        self.tags[project["repository"]] = ("v" if current.startswith("v") else "") + release
        self.commits[project["repository"]] = "b" * 40
        tailscale_path = self.root / "package/tailscale-rs/Makefile"
        tailscale_before = tailscale_path.read_bytes()
        (self.root / "package" / name / "files").mkdir()
        updater.update(self.root)
        text = (self.root / "package" / name / "Makefile").read_text(encoding="utf-8")
        self.assertIn(f"PKG_VERSION:={release}\n", text)
        self.assertIn("PKG_RELEASE:=1\n", text)
        self.assertIn("PKG_HASH:=" + "a" * 64 + "\n", text)
        self.assertIn("PKG_SOURCE_COMMIT:=" + "b" * 40 + "\n", text)
        self.assertEqual((self.root / "package" / name / "files/Cargo.lock").read_bytes(), b"new lock\n")
        self.assertEqual(tailscale_path.read_bytes(), tailscale_before)
        self.updated_source.assert_called_once()

    def test_new_tailscale_release_uses_the_release_commit(self):
        name = "tailscale-rs"
        repo = updater.PROJECTS[name]["repository"]
        current = self.tags[repo]
        major, minor, patch_version = updater.version(current.removeprefix("v"))
        release = f"{major}.{minor}.{patch_version + 1}"
        self.tags[repo] = f"v{release}"
        self.commits[repo] = "b" * 40
        (self.root / "package" / name / "files").mkdir()
        updater.update(self.root)
        recipe = (self.root / "package" / name / "Makefile").read_text(encoding="utf-8")
        self.assertIn(f"PKG_VERSION:={release}\n", recipe)
        self.assertIn("PKG_SOURCE_COMMIT:=" + "b" * 40 + "\n", recipe)
        self.assertIn("PKG_HASH:=" + "a" * 64 + "\n", recipe)
        self.updated_source.assert_called_once()
        self.assertEqual(self.updated_source.call_args.args[1:3], (name, f"v{release}"))
        self.assertEqual(self.updated_source.call_args.args[5], "b" * 40)

    def test_preview_replays_only_requested_package_at_its_pinned_revision(self):
        for name in updater.PROJECTS:
            with self.subTest(package=name):
                path = self.root / "package" / name / "Makefile"
                original = path.read_text(encoding="utf-8")
                digest = updater.match_one(r"^PKG_HASH:=([a-f0-9]{64})$", original).group(1)
                self.updated_source.return_value = (digest, b"replay lock\n", {})
                other = next(item for item in updater.PROJECTS if item != name)
                other_path = self.root / "package" / other / "Makefile"
                other_before = other_path.read_bytes()
                (path.parent / "files").mkdir(exist_ok=True)
                updater.update(self.root, replay=name)
                self.assertEqual(self.updated_source.call_args.args[5], self.commits[updater.PROJECTS[name]["repository"]])
                self.assertEqual(other_path.read_bytes(), other_before)
        self.latest_tag.assert_not_called()
        self.latest_openwrt.assert_not_called()

    def test_independent_lanes_qualify_simultaneous_releases(self):
        for name, project in updater.PROJECTS.items():
            repo = project["repository"]
            major, minor, micro = updater.version(self.tags[repo].removeprefix("v"))
            self.tags[repo] = f"v{major}.{minor}.{micro + 1}"
            self.commits[repo] = "b" * 40
            (self.root / "package" / name / "files").mkdir()
        before = self.snapshot()
        standard = updater.update(self.root, lane="standard")
        self.assertTrue(standard[0].startswith("protun-openwrt "))
        self.assertEqual(self.snapshot()[Path("package/tailscale-rs/Makefile")],
                         before[Path("package/tailscale-rs/Makefile")])
        self.updated_source.reset_mock()
        tailscale = updater.update(self.root, lane="tailscale")
        self.assertTrue(tailscale[0].startswith("tailscale-rs "))
        self.updated_source.assert_called_once()
        self.assertEqual(self.updated_source.call_args.args[1], "tailscale-rs")

    def test_lock_refresh_keeps_source_pin_and_increments_recipe_only_on_change(self):
        package = self.root / "package/tailscale-rs"
        lock = package / "files/Cargo.lock"
        lock.parent.mkdir()
        lock.write_bytes(b"baseline\n")
        recipe = package / "Makefile"
        original = recipe.read_text(encoding="utf-8")
        updates = [("rustls@0.23.45", "0.23.46")]
        updater.refresh_tailscale_lock(self.root, "cargo", updates)
        digest = updater.match_one(r"^PKG_HASH:=([a-f0-9]{64})$", original).group(1)
        self.assertEqual(self.updated_source.call_args.kwargs,
                         {"lock_updates": updates, "expected_digest": digest})
        number = updater.match_one(r"^PKG_RELEASE:=(\d+)$", original).group(1)
        expected = original.replace(f"PKG_RELEASE:={number}\n", f"PKG_RELEASE:={int(number) + 1}\n")
        self.assertEqual(recipe.read_text(encoding="utf-8"), expected)
        updater.refresh_tailscale_lock(self.root, "cargo", updates)
        self.assertEqual(recipe.read_text(encoding="utf-8"), expected)

    def test_retargeted_pinned_release_is_rejected(self):
        before = self.snapshot()
        for repo, revision in list(self.commits.items()):
            with self.subTest(repo=repo):
                self.commits[repo] = "c" * 40
                with self.assertRaisesRegex(ValueError, f"{repo} tag .* changed commit"):
                    updater.update(self.root)
                self.assertEqual(self.snapshot(), before)
                self.commits[repo] = revision


class AdapterScopeTests(unittest.TestCase):
    def test_adaptation_can_edit_a_client_but_cannot_change_the_release(self):
        import adapt_protunnext as agent

        client = agent.ADAPTER / "openwrt-client.rs"
        source = Path("release/src/api/types.rs")
        baseline = {client: b"client", source: b"upstream"}
        self.assertEqual(agent.changes(baseline, {**baseline, client: b"updated"}), {client})
        with self.assertRaisesRegex(ValueError, "outside"):
            agent.changes(baseline, {**baseline, source: b"changed"})
        with self.assertRaisesRegex(ValueError, "outside"):
            agent.changes(baseline, {source: b"upstream"})

    def test_tailscale_adaptation_reads_pinned_context_but_preserves_recipe(self):
        import adapt_protunnext as agent

        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            release = root / "release"
            release.mkdir()
            (release / "CHANGELOG.md").write_text("New C option", encoding="utf-8")
            (root / "AGENTS.md").write_text("Verify pinned claims", encoding="utf-8")
            recipe = Path("package/tailscale-rs/Makefile")
            for path in (*agent.TAILSCALE_EDITABLE, recipe):
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_text("baseline", encoding="utf-8")
            editable_path = Path("package/tailscale-rs/files/stage-header.py")

            def suggest(key, prompt, workspace, editable):
                self.assertIn("tailscale-rs", prompt)
                self.assertEqual((workspace / "release/CHANGELOG.md").read_text(encoding="utf-8"), "New C option")
                self.assertEqual((workspace / "AGENTS.md").read_text(encoding="utf-8"), "Verify pinned claims")
                self.assertTrue((workspace / recipe).is_file())
                self.assertNotIn(recipe.as_posix(), editable)
                (workspace / editable_path).write_text("update", encoding="utf-8")

            with patch.object(agent.opencode_ci, "run", side_effect=suggest):
                changes = agent.adapt(root, release, "v1.2.3", "0" * 40, "fixture", package="tailscale-rs")
            self.assertEqual(changes, {editable_path: b"update"})


if __name__ == "__main__":
    unittest.main()
