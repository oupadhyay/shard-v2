import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "updater", Path(__file__).with_name("update-split-revisions.py")
)
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)

A, B = "a" * 40, "b" * 40


def manifest(revision=A):
    return ('[dependencies]\nshard-tool-api = { git = "'
            + updater.OWNER + 'shard-tool-api", rev = "' + revision + '" }\n')


class SplitRevisionTests(unittest.TestCase):
    def test_replaces_only_requested_pin(self):
        text = manifest() + '\n[package]\nname = "example"\nversion = "0.1.0"\n'
        self.assertEqual(updater.replace_pin(text, "shard-tool-api", B), text.replace(A, B))

    def test_rejects_moving_or_wrong_sources(self):
        for text in (manifest("main"), manifest().replace("oupadhyay", "other"),
                     manifest().replace('rev =', 'branch =')):
            with self.assertRaises(ValueError):
                updater.pin(text, "shard-tool-api")

    def test_duplicate_nominal_sources_rejected(self):
        package = {"name": "shard-tool-api", "source": f"git+{updater.OWNER}shard-tool-api?rev={A}#{A}"}
        updater.audit_graph({"packages": [package]}, {"shard-tool-api": A})
        with self.assertRaises(ValueError):
            updater.audit_graph({"packages": [package, package]}, {"shard-tool-api": A})

    def test_host_waits_for_both_consumers(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(updater, "run", return_value=B):
            root = Path(tmp)
            for name in updater.SPLITS[1:]:
                (root / name).mkdir()
                (root / name / "Cargo.toml").write_text(manifest(A))
            self.assertEqual(updater.planned_pins("shard-provider", root), {"shard-tool-api": B})
            with self.assertRaisesRegex(ValueError, "Waiting"):
                updater.planned_pins("shard-v2", root)
            for name in updater.SPLITS[1:]:
                (root / name / "Cargo.toml").write_text(manifest(B))
            self.assertEqual(updater.planned_pins("shard-v2", root), dict.fromkeys(updater.SPLITS, B))

    def test_prepare_updates_manifest_audit_and_checklist(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text(manifest())
            (root / "scripts").mkdir()
            audit = root / "scripts/audit_dependency_boundary.py"
            audit.write_text(f'TOOL_API_REVISION = "{A}"\n')
            metadata = json.dumps({"packages": [{"name": "shard-tool-api",
                "source": f"git+{updater.OWNER}shard-tool-api?rev={B}#{B}"}]})
            with patch.object(updater, "planned_pins", return_value={"shard-tool-api": B}), \
                    patch.object(updater, "run", return_value=metadata) as run:
                updater.prepare("shard-provider", root, root, root / "body.md")
            self.assertEqual(updater.pin((root / "Cargo.toml").read_text(), "shard-tool-api"), B)
            self.assertIn(B, audit.read_text())
            self.assertIn("No auto-merge", (root / "body.md").read_text())
            self.assertEqual(run.call_count, 3)

    def test_noop_does_not_resolve_or_write(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text(manifest())
            with patch.object(updater, "planned_pins", return_value={"shard-tool-api": A}), \
                    patch.object(updater, "run") as run:
                updater.prepare("shard-provider", root, root, root / "body.md")
            run.assert_not_called()
            self.assertFalse((root / "body.md").exists())


if __name__ == "__main__":
    unittest.main()
