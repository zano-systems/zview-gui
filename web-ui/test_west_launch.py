"""`west zview-gui` helpers and the adapter's auto-connect startup path."""

import asyncio
import io
import json
import socket
import tempfile
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch
from urllib.request import urlopen

import server
from aiohttp import web
from console import Console
from session import SessionController
from test_build_context import make_build
from west_launch import data_root_for, heap_period, live_request, site_ready, zview_pin

ROOT = Path(__file__).resolve().parent.parent


class HelperTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.top = Path(temp.name).resolve()
        self.build = make_build(self.top)

    def test_pin_matches_the_adapter_requirement(self):
        pin = zview_pin()
        self.assertRegex(pin, r"^\d+\.\d+\.\d+$")
        self.assertIn(f"zview=={pin}", (ROOT / "web-ui/requirements.txt").read_text())
        bad = self.top / "requirements.txt"
        bad.write_text("aiohttp==1\nzview @ git+https://example.invalid/zview.git@abc1234\n")
        with self.assertRaises(ValueError):
            zview_pin(bad)

    def test_site_needs_page_and_wasm(self):
        site = self.top / "web"
        self.assertFalse(site_ready(site))
        (site / "pkg").mkdir(parents=True)
        (site / "index.html").write_text("<html></html>")
        self.assertFalse(site_ready(site))
        (site / "pkg" / "zview_desktop_bg.wasm").write_bytes(b"\0asm")
        self.assertTrue(site_ready(site))

    def test_heap_maps_follow_polling_unless_set(self):
        self.assertEqual(heap_period(0.1, None), 0.1)
        self.assertEqual(heap_period(0.25, None), 0.25)
        self.assertGreater(heap_period(0, None), 0)
        self.assertEqual(heap_period(0.1, 2.0), 2.0)

    def test_workspace_root_holds_build_and_sources(self):
        self.assertEqual(data_root_for(self.build, self.top), self.top)

    def test_outside_workspace_uses_common_ancestor_of_build_and_sources(self):
        other = self.top / "other-workspace"
        other.mkdir()
        self.assertEqual(data_root_for(self.build, other), self.top)
        self.assertEqual(data_root_for(self.build), self.top)

    def test_unrelated_sources_fall_back_to_the_build_only(self):
        (self.build / "CMakeCache.txt").write_text("APPLICATION_SOURCE_DIR:PATH=/\n")
        self.assertEqual(data_root_for(self.build, self.top / "missing"), self.build)

    def test_extra_paths_widen_the_root(self):
        elf = self.top / "elsewhere" / "app.elf"
        elf.parent.mkdir()
        elf.write_bytes(b"")
        self.assertEqual(data_root_for(self.build, self.build, [elf]), self.top)

    def test_live_request_is_root_relative_and_drops_unset_overrides(self):
        elf = self.build / "zephyr" / "zephyr.elf"
        self.assertEqual(live_request(self.top, self.build), {"build_dir": "build"})
        self.assertEqual(
            live_request(self.top, self.build, runner="pyocd", elf=elf, domain=None,
                         target="nrf5340_xxaa", frequency=1000000),
            {"build_dir": "build", "elf": "build/zephyr/zephyr.elf", "runner": "pyocd",
             "target": "nrf5340_xxaa", "frequency": 1000000},
        )
        self.assertEqual(live_request(self.build, self.build), {"build_dir": "."})
        with self.assertRaises(ValueError):
            live_request(self.build / "zephyr", self.build)


class ServeTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        make_build(self.root)

    async def run_serve(self, live, start_live):
        stop = asyncio.Event()
        ready = asyncio.Future()
        with patch.object(SessionController, "start_live", start_live), \
                patch("server.webbrowser.open") as opened:
            task = asyncio.create_task(server.serve(
                self.root, port=0, live=live, open_browser=True,
                ready=ready.set_result, stop=stop,
            ))
            url = await asyncio.wait_for(ready, 10)
            body = await asyncio.to_thread(
                lambda: json.loads(urlopen(url + "api/session", timeout=5).read())
            )
            stop.set()
            await asyncio.wait_for(task, 10)
        opened.assert_called_once_with(url)
        return url, body

    async def test_auto_connects_the_resolved_build_on_a_free_port(self):
        start_live = AsyncMock(return_value="{}")
        url, body = await self.run_serve({"build_dir": "build", "runner": "pyocd"}, start_live)
        self.assertRegex(url, r"^http://127\.0\.0\.1:\d+/$")
        self.assertNotEqual(url, "http://127.0.0.1:0/")
        self.assertEqual(body["type"], "snapshot")
        start_live.assert_awaited_once()
        target, source, elf_label, context = start_live.await_args.args
        self.assertEqual(target.runner, "pyocd")
        self.assertEqual(target.target, "nrf5340_xxaa")
        self.assertEqual(source["configuration"], {"build_dir": "build", "runner": "pyocd"})
        self.assertEqual(elf_label, "build/zephyr/zephyr.elf")
        self.assertEqual(context["board"], "nrf5340dk/nrf5340/cpuapp")

    async def test_invalid_build_keeps_serving_for_manual_configuration(self):
        start_live = AsyncMock()
        with patch("sys.stderr") as stderr:
            _, body = await self.run_serve({"build_dir": "missing"}, start_live)
        start_live.assert_not_awaited()
        self.assertIsNone(body["data"])
        self.assertIn("auto-connect failed", "".join(c.args[0] for c in stderr.write.call_args_list))

    async def test_taken_default_port_falls_back_but_an_explicit_one_fails(self):
        with socket.socket() as taken:
            taken.bind(("127.0.0.1", 0))
            taken.listen()
            busy = taken.getsockname()[1]
            with patch("server.DEFAULT_PORT", busy):
                stop = asyncio.Event()
                ready = asyncio.Future()
                task = asyncio.create_task(server.serve(
                    self.root, ready=ready.set_result, stop=stop))
                url = await asyncio.wait_for(ready, 10)
                stop.set()
                await asyncio.wait_for(task, 10)
                self.assertNotIn(f":{busy}/", url)
                with self.assertRaisesRegex(OSError, f"Cannot listen on 127.0.0.1:{busy}"):
                    await server.serve(self.root, port=busy, stop=asyncio.Event())

    async def serve_beside(self, payload):
        """Serve Live while another service holding DEFAULT_PORT answers ``payload``."""
        other = web.Application()
        other.router.add_get("/api/session", lambda _: web.json_response(payload))
        runner = web.AppRunner(other)
        await runner.setup()
        site = web.TCPSite(runner, "127.0.0.1", 0)
        await site.start()
        busy = runner.addresses[0][1]
        out = io.StringIO()
        try:
            with patch("server.DEFAULT_PORT", busy), \
                    patch.object(SessionController, "start_live", AsyncMock()):
                stop, ready = asyncio.Event(), asyncio.Future()
                task = asyncio.create_task(server.serve(
                    self.root, live={"build_dir": "build", "runner": "pyocd"},
                    ready=ready.set_result, stop=stop, console=Console(out, color=False)))
                url = await asyncio.wait_for(ready, 10)
                stop.set()
                await asyncio.wait_for(task, 10)
        finally:
            await runner.cleanup()
        return busy, url, out.getvalue()

    async def test_warns_when_another_session_polls_the_same_probe(self):
        busy, _, out = await self.serve_beside({"type": "snapshot", "data": {
            "source": {"mode": "live"}, "status": "reading",
            "context": {"runner": "pyocd", "target": "nrf5340_xxaa", "board": "other-board"},
        }})
        self.assertIn(f"another ZView GUI session (http://127.0.0.1:{busy}/) is already "
                      "reading other-board via pyocd", out)

    async def test_another_service_on_the_port_is_not_mistaken_for_a_session(self):
        busy, url, out = await self.serve_beside(["not", "a", "session"])
        self.assertNotIn(f":{busy}/", url)
        self.assertIn("using a free port", out)
        self.assertNotIn("another ZView GUI session", out)

    async def test_without_live_nothing_connects(self):
        start_live = AsyncMock()
        await self.run_serve(None, start_live)
        start_live.assert_not_awaited()


class SharedProbeTests(unittest.TestCase):
    CONTEXT = {"runner": "jlink", "target": "nRF52833_xxAA", "board": "nrf52833dk"}

    def snapshot(self, mode="live", status="reading", **context):
        return {"data": {"source": {"mode": mode}, "status": status,
                         "context": {**self.CONTEXT, **context}}}

    def test_same_runner_and_target_while_live(self):
        self.assertEqual(server.shared_probe(self.snapshot(), self.CONTEXT), "nrf52833dk")
        self.assertEqual(server.shared_probe(self.snapshot(status="opening"), self.CONTEXT),
                         "nrf52833dk")

    def test_other_probes_replays_and_idle_sessions_do_not_warn(self):
        for snapshot in (
            self.snapshot(target="nRF5340_xxAA_APP"), self.snapshot(runner="pyocd"),
            self.snapshot(mode="replay"), self.snapshot(status="stopped"),
            {"type": "snapshot", "data": None}, None,
        ):
            self.assertIsNone(server.shared_probe(snapshot, self.CONTEXT), snapshot)

    def test_other_services_json_is_not_a_session(self):
        for snapshot in (
            [], ["live"], "reading", 42, {"data": []}, {"data": "x"},
            {"data": {"source": "live", "status": "reading"}},
            {"data": {"source": {"mode": "live"}, "status": ["reading"], "context": self.CONTEXT}},
            {"data": {"source": {"mode": "live"}, "status": "reading", "context": [1]}},
        ):
            self.assertIsNone(server.shared_probe(snapshot, self.CONTEXT), snapshot)
        odd_board = self.snapshot(board=["nrf52833dk"])
        self.assertEqual(server.shared_probe(odd_board, self.CONTEXT), "the same target")


if __name__ == "__main__":
    unittest.main()
