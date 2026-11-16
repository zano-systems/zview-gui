"""The zview-gui package metadata, pins and the two command lines agree."""

import argparse
import importlib.util
import re
import sys
import types
import unittest
from pathlib import Path
from unittest.mock import patch

import tomllib
from zview_gui import cli

ROOT = Path(__file__).resolve().parent.parent
PYPROJECT = tomllib.loads((ROOT / "pyproject.toml").read_text())


def pinned(text, name):
    """The exact version ``name==X`` pins in a requirements text or list."""
    lines = text if isinstance(text, list) else text.splitlines()
    for line in lines:
        match = re.fullmatch(rf"\s*{re.escape(name)}\s*==\s*([0-9][0-9A-Za-z.+-]*)\s*", line)
        if match:
            return match.group(1)
    raise ValueError(f"No exact {name} pin.")


def zview_pin():
    """The ZView release the adapter is tested with."""
    return pinned((ROOT / "web-ui" / "requirements.txt").read_text(), "zview")


def load_west_command():
    """``zview_gui_cmd`` with a stub ``west.commands``, so west need not be installed."""
    stub = types.ModuleType("west.commands")
    stub.WestCommand = type("WestCommand", (), {"__init__": lambda self, *args: None})
    spec = importlib.util.spec_from_file_location(
        "zview_gui_cmd", ROOT / "scripts" / "west_commands" / "zview_gui_cmd.py")
    module = importlib.util.module_from_spec(spec)
    with patch.dict(sys.modules, {"west": types.ModuleType("west"), "west.commands": stub}):
        spec.loader.exec_module(module)
    return module


def options(parser):
    return {(tuple(action.option_strings), action.dest, action.default, action.type)
            for action in parser._actions if action.option_strings and action.dest != "help"}


class PackagingTests(unittest.TestCase):
    def test_one_version_everywhere(self):
        version = PYPROJECT["project"]["version"]
        cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        self.assertEqual(version, cargo)
        west = load_west_command()
        self.assertEqual(west.required_version(), version)

    def test_dependencies_pin_the_tested_zview(self):
        dependencies = PYPROJECT["project"]["dependencies"]
        self.assertEqual(pinned(dependencies, "zview"), zview_pin())
        names = {re.match(r"[A-Za-z0-9_.-]+", dep).group(0).lower() for dep in dependencies}
        self.assertEqual(names, {"zview", "aiohttp", "pyyaml"})
        tested = (ROOT / "web-ui" / "requirements.txt").read_text()
        # The exact versions CI runs must satisfy the ranges the wheel declares.
        self.assertTrue(pinned(tested, "aiohttp").startswith("3."))
        self.assertTrue(pinned(tested, "PyYAML").startswith("6."))

    def test_console_script_is_the_cli(self):
        self.assertEqual(PYPROJECT["project"]["scripts"], {"zview-gui": "zview_gui.cli:main"})

    def test_west_and_standalone_share_the_serving_options(self):
        west = load_west_command()
        west_parser = argparse.ArgumentParser()
        command = west.ZViewGuiCommand()
        command.name, command.help, command.description = "zview-gui", "", ""
        sub = west_parser.add_subparsers()
        west_options = options(command.do_add_parser(sub))
        shared = argparse.ArgumentParser()
        cli.add_serve_options(shared)
        rebuild = {opt for opt in west_options if opt[1] == "rebuild"}
        self.assertEqual(len(rebuild), 1)
        self.assertEqual(west_options - rebuild, options(shared))

    def test_bad_requirement_files_are_reported(self):
        west = load_west_command()
        with self.assertRaises(ValueError):
            west.required_version(ROOT / "web-ui" / "requirements.txt")
        with self.assertRaises(ValueError):
            pinned("zview @ git+https://example.invalid/zview.git@abc1234\n", "zview")


if __name__ == "__main__":
    unittest.main()
