"""Terminal console: levels in colour, and session events reported once."""

import asyncio
import io
import unittest
from unittest.mock import patch

from console import RESET, STYLES, Console, wants_color
from session import SessionController


def plain():
    out = io.StringIO()
    return Console(out, color=False), out


class ConsoleTests(unittest.TestCase):
    def test_levels_are_coloured_and_urls_highlighted(self):
        out = io.StringIO()
        console = Console(out, color=True)
        console.error("boom")
        console.serving("http://127.0.0.1:8765/")
        text = out.getvalue()
        self.assertIn(f"{STYLES['error']}boom{RESET}", text)
        self.assertIn(f"{STYLES['url']}http://127.0.0.1:8765/{RESET}", text)
        self.assertIn(f"{STYLES['label']}ZView GUI:{RESET}", text)

    def test_plain_output_has_no_escape_codes(self):
        console, out = plain()
        console.field("Build directory", "/w/build")
        console.serving("http://127.0.0.1:1/")
        self.assertNotIn("\033", out.getvalue())
        self.assertIn("ZView GUI: Build directory: /w/build\n", out.getvalue())
        self.assertIn("ZView GUI: open http://127.0.0.1:1/\n", out.getvalue())

    def test_color_follows_terminal_and_environment(self):
        tty = io.StringIO()
        tty.isatty = lambda: True
        with patch.dict("os.environ", {}, clear=True):
            self.assertTrue(wants_color(tty))
            self.assertFalse(wants_color(io.StringIO()))
        with patch.dict("os.environ", {"NO_COLOR": "1"}, clear=True):
            self.assertFalse(wants_color(tty))
        with patch.dict("os.environ", {"FORCE_COLOR": "1"}, clear=True):
            self.assertTrue(wants_color(io.StringIO()))

    def test_status_changes_are_reported_once(self):
        console, out = plain()
        for status in ("reading", "reading", "stopping", "lost", "reading"):
            console.session_event("status", {"status": status, "error": None})
        console.session_event("frame", {"groups": {}})
        self.assertEqual(out.getvalue().splitlines(), [
            "ZView GUI: Live reading frames",
            "ZView GUI: Live connection lost",
            "ZView GUI: Live reading frames",
        ])
        console.session_event("status", {"status": "failed", "error": "probe gone"})
        self.assertTrue(out.getvalue().endswith("ZView GUI: Live failed: probe gone\n"))

    def test_repeated_notices_are_rate_limited(self):
        now = [0.0]
        out = io.StringIO()
        console = Console(out, color=False, clock=lambda: now[0])
        for at in (0.0, 1.0, 29.0, 31.0):
            now[0] = at
            console.session_event("notice", {"message": "read fault"})
        console.session_event("notice", {"message": "other fault"})
        self.assertEqual(out.getvalue().splitlines(), [
            "ZView GUI: read fault", "ZView GUI: read fault", "ZView GUI: other fault",
        ])

    def test_controller_reports_processed_events(self):
        seen = []

        async def run():
            controller = SessionController()
            controller.observer = lambda kind, data: seen.append((kind, data["status"]))
            controller.session = type("S", (), {"id": "s1", "status": "opening",
                                                "error": None, "samples_per_second": 1})()
            await controller._event("s1", "status", {"status": "reading"})
            await controller._event("other", "status", {"status": "lost"})

        asyncio.run(run())
        self.assertEqual(seen, [("status", "reading")])


if __name__ == "__main__":
    unittest.main()
