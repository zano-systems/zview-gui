"""Tests for the local launcher; no Rust build or browser is needed."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import web_ui
from web_ui import ROOT


def setUpModule():
    (ROOT / "target").mkdir(exist_ok=True)


class AdapterLauncherTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=ROOT / "target")
        self.addCleanup(self.directory.cleanup)
        self.site = Path(self.directory.name)
        for name in ("index.html", "main.js", "startup.mjs", "pkg/zview_desktop.js",
                     "pkg/zview_desktop_bg.wasm", "fonts/OFL.txt"):
            path = self.site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"build output")
        self.site_patch = patch.object(web_ui, "SITE", self.site)
        self.site_patch.start()
        self.addCleanup(self.site_patch.stop)

    def test_launches_selected_installed_adapter_with_this_checkouts_site(self):
        with patch.object(web_ui, "build") as build, patch.object(web_ui.subprocess, "run") as run:
            web_ui.serve(["--no-open", "--data-root", "capture directory"], python="adapter python")
        build.assert_called_once_with()
        self.assertEqual(run.call_args_list[0].args[0],
                         ["adapter python", "-I", "-c", "import zview_gui, zview"])
        self.assertEqual(run.call_args.args[0], [
            "adapter python", "-I", "-m", "zview_gui", "--no-open",
            "--data-root", "capture directory", "--site", str(self.site),
        ])
        self.assertTrue(run.call_args.kwargs["check"])

    def test_skip_build_still_requires_adapter_and_complete_site(self):
        with patch.object(web_ui, "build") as build, patch.object(web_ui.subprocess, "run") as run:
            with patch.dict("os.environ", {"ZVIEW_GUI_PYTHON": "configured-python"}):
                web_ui.serve(skip_build=True)
            build.assert_not_called()
            self.assertEqual(run.call_args.args[0][0], "configured-python")
            (self.site / "startup.mjs").unlink()
            with self.assertRaisesRegex(ValueError, "complete browser build"):
                web_ui.serve(skip_build=True)

    def test_missing_adapter_fails_before_build_without_static_fallback(self):
        import subprocess
        with patch.object(web_ui, "build") as build, patch.object(
            web_ui.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "python"),
        ) as run:
            with self.assertRaisesRegex(ValueError, "required Python adapter"):
                web_ui.serve()
        build.assert_not_called()
        self.assertEqual(run.call_count, 1)

    def test_cli_forwards_adapter_options_without_consuming_their_values(self):
        with patch("sys.argv", ["web_ui.py", "serve", "--python", "env/bin/python",
                                "--skip-build", "--port", "0", "--no-open"]), \
                patch.object(web_ui, "serve") as serve:
            self.assertEqual(web_ui.main(), 0)
        serve.assert_called_once_with(["--port", "0", "--no-open"],
                                      python="env/bin/python", skip_build=True)


class WasmOptTests(unittest.TestCase):
    def test_release_modules_go_through_wasm_opt_with_pinned_flags(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
            log = Path(directory) / "args.json"
            fake = Path(directory) / "wasm-opt"
            # Records its arguments and halves the module, like a real optimizer.
            fake.write_text(
                "#!/usr/bin/env python3\nimport json, sys\n"
                f"open({str(log)!r}, 'w').write(json.dumps(sys.argv[1:]))\n"
                "data = open(sys.argv[-3], 'rb').read()\n"
                "open(sys.argv[-1], 'wb').write(data[: len(data) // 2])\n")
            fake.chmod(0o755)
            wasm = Path(directory) / "app.wasm"
            wasm.write_bytes(b"\0asm" + b"x" * 96)
            with patch.dict("os.environ", {"WASM_OPT": str(fake)}):
                web_ui.optimize(wasm)
            args = json.loads(log.read_text())
            self.assertEqual(args[0], web_ui.WASM_OPT_LEVEL)
            self.assertIn("--enable-bulk-memory", args)
            self.assertIn("--enable-threads", args)  # the module has atomic instructions
            self.assertIn("--strip-debug", args)
            self.assertEqual(args[-3:], [str(wasm), "-o", str(wasm)])
            self.assertEqual(wasm.stat().st_size, 50)

    def test_missing_wasm_opt_is_skipped_unless_required(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
            wasm = Path(directory) / "app.wasm"
            wasm.write_bytes(b"\0asm")
            with patch.object(web_ui, "wasm_opt", return_value=None):
                with patch.dict("os.environ", {}, clear=False), patch("builtins.print"):
                    web_ui.optimize(wasm)
                self.assertEqual(wasm.read_bytes(), b"\0asm")
                with patch.dict("os.environ", {"ZVIEW_REQUIRE_WASM_OPT": "1"}):
                    with self.assertRaisesRegex(ValueError, "wasm-opt is required"):
                        web_ui.optimize(wasm)


if __name__ == "__main__":
    unittest.main()
