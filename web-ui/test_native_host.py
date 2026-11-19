"""The native sidecar uses the real adapter and stops when its parent closes stdin."""

import asyncio
import json
import sys
import tempfile
import unittest

from aiohttp import ClientSession
from zview_gui.session import PROTOCOL_VERSION


class NativeHostTests(unittest.IsolatedAsyncioTestCase):
    async def test_readiness_api_and_parent_eof_without_browser_assets(self):
        with tempfile.TemporaryDirectory(prefix="native root ") as root:
            process = await asyncio.create_subprocess_exec(
                sys.executable, "-I", "-u", "-m", "zview_gui.native_host",
                "--data-root", root,
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            try:
                line = await asyncio.wait_for(process.stdout.readline(), 15)
                self.assertTrue(line, "The sidecar exited before readiness.")
                ready = json.loads(line)
                self.assertEqual(ready["protocol"], PROTOCOL_VERSION)
                self.assertTrue(ready["package_version"])
                self.assertTrue(ready["url"].startswith("http://127.0.0.1:"))
                async with ClientSession() as client:
                    async with client.get(ready["url"] + "api/session") as response:
                        self.assertEqual(response.status, 200)
                    async with client.ws_connect(
                        ready["url"] + "api/events?client_id=native-test"
                    ) as socket:
                        snapshot = await asyncio.wait_for(socket.receive_json(), 5)
                        self.assertEqual(snapshot["type"], "snapshot")
                        self.assertEqual(snapshot["version"], PROTOCOL_VERSION)
                    # Native remains subject to the browser adapter's security rules.
                    async with client.post(
                        ready["url"] + "api/build/resolve",
                        json={"mode": "live", "build_dir": root},
                        headers={"Origin": "https://example.invalid"},
                    ) as response:
                        self.assertEqual(response.status, 403)
                process.stdin.close()
                await asyncio.wait_for(process.wait(), 10)
                diagnostics = (await process.stderr.read()).decode()
                self.assertEqual(process.returncode, 0, diagnostics)
                self.assertNotIn("open http://", diagnostics)
                self.assertNotIn("Ctrl+C", diagnostics)
                self.assertIn("stopping", diagnostics)
                self.assertEqual(await process.stdout.read(), b"")
            finally:
                if process.returncode is None:
                    process.kill()
                    await process.wait()


if __name__ == "__main__":
    unittest.main()
