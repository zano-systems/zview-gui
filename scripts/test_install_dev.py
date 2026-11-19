"""Development packaging tests; no Rust build, network or installation needed."""

from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import install_dev
from web_ui import ROOT


class DevelopmentInstallTests(unittest.TestCase):
    def test_staging_changes_only_version_and_copies_packaging_inputs(self):
        original = (ROOT / "pyproject.toml").read_bytes()
        with tempfile.TemporaryDirectory() as directory:
            stage = Path(directory)
            version = install_dev.stage_project(ROOT, stage)
            self.assertRegex(version, r"^\d+\.\d+\.\d+\.dev0$")
            self.assertEqual(
                (stage / "pyproject.toml").read_bytes(),
                original.replace(
                    f'version = "{version.removesuffix(".dev0")}"'.encode(),
                    f'version = "{version}"'.encode()))
            for name in ("hatch_build.py", "LICENSE", "NOTICE", "web-ui/PYPI.md",
                         "web-ui/zview_gui/cli.py"):
                self.assertEqual((stage / name).read_bytes(), (ROOT / name).read_bytes())
            self.assertFalse((stage / "web-ui/zview_gui/site").exists())
            self.assertFalse(list(stage.rglob("__pycache__")))
        self.assertEqual((ROOT / "pyproject.toml").read_bytes(), original)

    def test_wheel_uses_selected_python_and_absolute_site_and_is_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "dist"
            with patch.object(install_dev.subprocess, "run") as run, \
                    patch.object(install_dev, "check_contents") as check:
                wheel = install_dev.build_wheel(ROOT, root / "site", output)
            args, kwargs = run.call_args
            self.assertEqual(args[0][:4], [install_dev.sys.executable, "-m", "pip", "wheel"])
            self.assertIn("--no-deps", args[0])
            self.assertIn("--no-cache-dir", args[0])
            self.assertEqual(kwargs["env"]["ZVIEW_GUI_SITE"], str((root / "site").resolve()))
            self.assertTrue(kwargs["check"])
            self.assertEqual(wheel.parent, output)
            check.assert_called_once_with(wheel)
            self.assertFalse(Path(args[0][-1]).exists())

    def test_non_virtual_environment_is_rejected_before_build(self):
        with patch.object(install_dev.sys, "prefix", install_dev.sys.base_prefix), \
                patch.object(install_dev, "build") as build:
            with self.assertRaisesRegex(SystemExit, "virtual environment"):
                install_dev.main()
            build.assert_not_called()

    def test_install_replaces_previous_dev_wheel_and_checks_dependencies(self):
        with patch.object(install_dev.sys, "prefix", "/virtual-env"), \
                patch.object(install_dev.sys, "base_prefix", "/base"), \
                patch.object(install_dev, "build") as build, \
                patch.object(install_dev, "build_wheel", return_value=Path("dev.whl")), \
                patch.object(install_dev.subprocess, "run") as run:
            install_dev.main()
        build.assert_called_once_with(release=True)
        self.assertEqual(run.call_args_list[0].args[0], [
            install_dev.sys.executable, "-m", "pip", "install",
            "--force-reinstall", "dev.whl"])
        self.assertEqual(run.call_args_list[1].args[0][-2:], ["pip", "check"])
        self.assertEqual(run.call_args_list[2].args[0][-2:], ["zview_gui", "--version"])


if __name__ == "__main__":
    unittest.main()
