"""Regression checks for browser interaction synchronization (no browser needed)."""

import asyncio
import importlib.util
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch


@unittest.skipUnless(importlib.util.find_spec("playwright"), "requires browser test dependencies")
class SidebarToggleTests(unittest.IsolatedAsyncioTestCase):
    async def check_toggle(self, collapsed):
        import browser_smoke

        current = {
            "sidebar_collapsed": collapsed,
            "sidebar_width": 300,
            "session_panel_bounds": {"height": 86},
        }
        page = SimpleNamespace(
            mouse=SimpleNamespace(move=AsyncMock(), click=AsyncMock()),
            evaluate=AsyncMock(),
            wait_for_timeout=AsyncMock(),
        )
        waiting = asyncio.Event()
        painted = asyncio.Event()

        async def wait_for_state(page, predicate, description):
            if description == "sidebar layout before toggle":
                self.assertFalse(predicate({
                    **current, "session_panel_bounds": {"height": 0},
                }))
                self.assertTrue(predicate(current))
                return current
            self.assertFalse(predicate(current))
            waiting.set()
            await painted.wait()
            updated = {**current, "sidebar_collapsed": not collapsed}
            self.assertTrue(predicate(updated))
            return updated

        with patch.object(browser_smoke, "wait_state", side_effect=wait_for_state):
            task = asyncio.create_task(browser_smoke.toggle_sidebar(page))
            try:
                await asyncio.wait_for(waiting.wait(), timeout=1)
                self.assertFalse(task.done(), "The helper must wait for the state change.")
                page.mouse.click.assert_awaited_once_with(81 if collapsed else 327, 28)
                page.wait_for_timeout.assert_not_awaited()
            finally:
                painted.set()
                await task

    async def test_collapse_waits_for_layout_and_result(self):
        await self.check_toggle(False)

    async def test_expand_waits_for_layout_and_result(self):
        await self.check_toggle(True)
