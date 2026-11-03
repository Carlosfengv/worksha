import importlib.util
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "cleanup", Path(__file__).resolve().parents[2] / "scripts/cleanup-test-apps.py"
)
cleanup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cleanup)


class CleanupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def app(self, name="Local Agent Review", scope=".work", identifier="app.localagent.review"):
        app = self.root / scope / (name + ".app")
        (app / "Contents/MacOS").mkdir(parents=True)
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps({
            "CFBundleIdentifier": identifier, "CFBundleExecutable": "review-desktop",
            "CFBundleName": name,
        }))
        for executable in ["review-desktop", "local-agent-service"]:
            (app / "Contents/MacOS" / executable).write_text("fixture executable")
        return app

    def plan(self, records=(), running=(), **kwargs):
        return cleanup.make_plan(self.root, records, set(map(str, running)), min_age_minutes=0, **kwargs)

    def apply(self, plan, records=(), running=(), command=None):
        with patch.object(cleanup, "running_apps", return_value=set(map(str, running))), \
             patch.object(cleanup, "registrations", return_value=[]), \
             patch.object(cleanup, "command", command or (lambda args: "")):
            cleanup.apply_plan(self.root, plan, records, lambda: None)

    def test_discovery_rejects_foreign_apps_and_symlinked_bundles_or_scopes(self):
        app = self.app()
        self.app("Foreign", identifier="app.localagentx.foreign")
        self.app("Outside", scope="user-data")
        (self.root / "output").symlink_to(self.root / "user-data", target_is_directory=True)
        (self.root / ".work/Linked.app").symlink_to(app, target_is_directory=True)
        self.assertEqual([a["path"] for a in cleanup.discover_apps(self.root)], [str(app)])
        self.assertFalse(cleanup.owned_path(self.root, self.root / ".work/Linked.app"))

    def test_live_bundle_is_kept_but_old_copy_with_same_identifier_is_removable(self):
        live = self.app("Local Agent", scope="target/debug/bundle/macos", identifier="app.localagent.desktop")
        old = self.app("Local Agent", scope="output/archive", identifier="app.localagent.desktop")
        plan = self.plan(running=[live])
        self.assertEqual([a["path"] for a in plan["remove"]], [str(old)])
        self.assertEqual(plan["preserve"][0]["reason"], "running")

    def test_sidecar_and_renamed_process_also_protect_bundle(self):
        app = self.app()
        output = f"  42 {app}/Contents/MacOS/renamed-review\n 43 {app}/Contents/MacOS/local-agent-service\n"
        self.assertEqual(cleanup.parse_running(output), {str(app)})

    def test_fresh_copies_are_kept_even_if_their_mtime_is_old(self):
        app = self.app()
        import os
        os.utime(app, (1, 1))
        plan = cleanup.make_plan(self.root, [], set(), now=time.time())
        self.assertEqual(plan["remove"], [])
        self.assertEqual(plan["preserve"][0]["reason"], "recently-created-or-changed")

    def test_targeted_cleanup_and_explicit_keep(self):
        first, second = self.app("First"), self.app("Second")
        plan = self.plan(selected=[first], keep=[first])
        self.assertEqual(plan["remove"], [])
        self.assertEqual({a["path"] for a in plan["preserve"]}, {str(first), str(second)})

    def test_only_localagent_registrations_are_parsed(self):
        block = "\n-----\nidentifier: {id}\npath: {path} (0xabc)\nname: anything\n"
        output = block.format(id="app.localagent.test", path="/tmp/Gone.app")
        output += block.format(id="app.localagentx.test", path="/tmp/Foreign.app")
        self.assertEqual(cleanup.parse_registrations(output), [{"path": "/tmp/Gone.app", "identifier": "app.localagent.test"}])

    def test_stale_registration_is_exact_and_existing_external_app_is_preserved(self):
        missing = self.root / "outside/Gone.app"
        existing = self.app("Existing", scope="outside")
        records = [{"path": str(p), "identifier": "app.localagent.test"} for p in [missing, existing]]
        plan = self.plan(records)
        calls = []
        self.apply(plan, records, command=lambda args: calls.append(args))
        self.assertEqual(calls, [[cleanup.LSREGISTER, "-u", str(missing)]])
        self.assertTrue(existing.exists())

    def test_removal_preserves_user_data_documents_and_symlink_targets(self):
        app = self.app()
        data = self.root / "user-data"
        data.mkdir()
        (data / "database.sqlite3").write_bytes(b"user database")
        document = self.root / "output/acceptance.md"
        document.parent.mkdir()
        document.write_text("acceptance evidence")
        (app / "Contents/data-link").symlink_to(data, target_is_directory=True)
        plan = self.plan()
        self.apply(plan)
        self.assertFalse(app.exists())
        self.assertEqual((data / "database.sqlite3").read_bytes(), b"user database")
        self.assertEqual(document.read_text(), "acceptance evidence")
        self.assertEqual(plan["removed"], [str(app)])
        self.assertEqual(len(plan["remove"][0]["binarySha256"]), 2)

    def test_bundle_changed_since_preview_is_not_removed(self):
        app = self.app()
        plan = self.plan()
        (app / "Contents/MacOS/local-agent-service").write_text("new build")
        self.apply(plan)
        self.assertTrue(app.exists())
        self.assertEqual(plan["skipped"][0]["reason"], "changed-or-running")

    def test_bundle_launched_after_preview_is_not_removed(self):
        app = self.app()
        plan = self.plan()
        self.apply(plan, running=[app])
        self.assertTrue(app.exists())
        self.assertEqual(plan["removed"], [])

    def test_failed_process_inspection_fails_closed(self):
        app = self.app()
        plan = self.plan()
        with patch.object(cleanup, "running_apps", side_effect=subprocess.CalledProcessError(1, ["ps"])), \
             patch.object(cleanup, "registrations", return_value=[]):
            with self.assertRaises(subprocess.CalledProcessError):
                cleanup.apply_plan(self.root, plan, [], lambda: None)
        self.assertTrue(app.exists())
        self.assertEqual(len(plan["errors"]), 1)

    def test_unregister_failure_is_reported_and_stops_before_next_bundle(self):
        app, next_app = self.app("First"), self.app("Second")
        record = {"path": str(app), "identifier": "app.localagent.review"}
        def fail(args):
            raise subprocess.CalledProcessError(1, args)
        plan = self.plan([record])
        with self.assertRaises(subprocess.CalledProcessError):
            self.apply(plan, [record], command=fail)
        self.assertFalse(app.exists())
        self.assertTrue(next_app.exists())
        self.assertEqual(plan["removed"], [str(app)])
        self.assertEqual(len(plan["errors"]), 1)

    def test_reappearing_stale_bundle_is_not_unregistered(self):
        app = self.root / ".work/Reappeared.app"
        record = {"path": str(app), "identifier": "app.localagent.review"}
        plan = self.plan([record])
        self.app("Reappeared")
        calls = []
        self.apply(plan, [record], command=lambda args: calls.append(args))
        self.assertTrue(app.exists())
        self.assertEqual(calls, [])
        self.assertEqual(plan["skipped"][0]["reason"], "reappeared-or-running")

    def test_default_cli_only_previews_and_writes_a_new_report(self):
        app = self.app()
        report = self.root / "output/report.json"
        with patch.object(cleanup, "ROOT", self.root), \
             patch.object(cleanup.sys, "platform", "darwin"), \
             patch.object(cleanup.sys, "argv", ["cleanup", "--report", str(report), "--min-age-minutes", "0"]), \
             patch.object(cleanup, "registrations", return_value=[]), \
             patch.object(cleanup, "running_apps", return_value=set()), \
             patch("builtins.print"):
            self.assertEqual(cleanup.main(), 0)
        self.assertTrue(app.exists())
        self.assertEqual(json.loads(report.read_text())["mode"], "preview")

    def test_app_starting_before_removal_is_kept_without_unregistering(self):
        app = self.app()
        record = {"path": str(app), "identifier": "app.localagent.review"}
        plan, calls = self.plan([record]), []
        with patch.object(cleanup, "running_apps", side_effect=[set(), {str(app)}, {str(app)}]), \
             patch.object(cleanup, "registrations", return_value=[record]), \
             patch.object(cleanup, "command", side_effect=lambda args: calls.append(args)):
            cleanup.apply_plan(self.root, plan, [record], lambda: None)
        self.assertTrue(app.exists())
        self.assertEqual(calls, [])
        self.assertEqual(plan["remainingTargetRegistrations"], [])

    def test_late_registration_of_a_removed_bundle_is_retried_by_exact_path(self):
        app = self.app()
        record = {"path": str(app), "identifier": "app.localagent.review"}
        plan, calls = self.plan(), []
        with patch.object(cleanup, "running_apps", return_value=set()), \
             patch.object(cleanup, "registrations", side_effect=[[record], [record], []]), \
             patch.object(cleanup, "command", side_effect=lambda args: calls.append(args)):
            cleanup.apply_plan(self.root, plan, [], lambda: None)
        self.assertFalse(app.exists())
        self.assertEqual(calls, [[cleanup.LSREGISTER, "-u", str(app)]])
        self.assertEqual(plan["registrationRetries"], [record])
        self.assertEqual(plan["remainingTargetRegistrations"], [])


if __name__ == "__main__":
    unittest.main()
