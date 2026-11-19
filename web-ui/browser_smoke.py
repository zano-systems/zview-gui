"""Exercise the real GPUI Rust/WASM client against the installed Python backend."""

import argparse
import asyncio
import json
import shutil
import sys
import tempfile
import threading
import time
from pathlib import Path

import yaml
from aiohttp import ClientSession, web
from playwright.async_api import async_playwright
from test_build_context import make_build
from test_native_dialog import ScriptedDialogs
from zview_gui.server import SOCKETS, create_app
from zview_gui.session import SessionController

ROOT = Path(__file__).resolve().parent.parent
RECORDING = "test-data/tests/fixtures/recordings/sys_heap_v4.3.ndjson.gz"
ELF = "test-data/tests/fixtures/zephyr.elf"


async def state(page):
    return await page.evaluate("""async () =>
        JSON.parse((await import('/pkg/zview_desktop.js')).adapter_state())""")


async def wait_state(page, predicate, description, timeout=20):
    deadline = asyncio.get_running_loop().time() + timeout
    last = None
    while asyncio.get_running_loop().time() < deadline:
        last = await state(page)
        if predicate(last):
            # State changes repaint on the next frame. Clicking before that hits
            # the previous frame (e.g. a still-disabled button), which optimized
            # builds reach first.
            await page.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))")
            return last
        await asyncio.sleep(0.1)
    raise AssertionError(f"Timed out waiting for {description}: {last}")


async def open_replay(page, elf=ELF):
    # Invoke the exact Rust command used by the GPUI button, not JS fetch.
    await page.evaluate("""async ([recording, elf]) =>
        (await import('/pkg/zview_desktop.js')).adapter_open_replay(recording, elf)""",
        [RECORDING, elf])


async def fill_canvas_input(page, y, text):
    """At the fixed desktop viewport, edit GPUI inputs through real key events."""
    await page.mouse.click(650, y)
    # Wait for GPUI to focus/sync its hidden browser text-input mirror.
    await page.wait_for_timeout(100)
    await page.keyboard.press("Control+A")
    await page.keyboard.press("Backspace")
    await page.keyboard.type(text, delay=2)
    await page.wait_for_timeout(100)


async def click_header(page, primary=False):
    """Click actual sidebar actions, opening the collapsed session popover first."""
    # A viewport/sidebar change reaches GPUI's canvas layout on a later frame.
    await page.wait_for_timeout(150)
    current = await state(page)
    if current["sidebar_collapsed"] and not current["session_popover_open"]:
        bounds = current["session_toggle_bounds"]
        await page.mouse.click(bounds["x"] + bounds["width"] / 2,
                               bounds["y"] + bounds["height"] / 2)
        await page.wait_for_timeout(200)
        current = await state(page)
    bounds = current["session_action_bounds"]
    assert bounds["width"] > 0 and bounds["height"] > 0, bounds
    panel = current["session_panel_bounds"]
    assert panel["x"] <= bounds["x"], current
    assert bounds["x"] + bounds["width"] <= panel["x"] + panel["width"], current
    await page.mouse.click(bounds["x"] + (bounds["width"] - 12 if primary else 12),
                           bounds["y"] + bounds["height"] / 2)


async def toggle_sidebar(page):
    # A restored session can be observable before its sidebar has been painted.
    # Wait for layout before sending a real click, then wait for its effect
    # rather than assuming the next GPUI frame arrives within 200 ms.
    current = await wait_state(
        page, lambda s: s["session_panel_bounds"]["height"] > 0,
        "sidebar layout before toggle",
    )
    collapsed = current["sidebar_collapsed"]
    sidebar = 54 if current["sidebar_collapsed"] else current["sidebar_width"]
    await page.mouse.move(sidebar + 27, 28)
    await page.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))")
    await page.mouse.click(sidebar + 27, 28)
    await wait_state(
        page, lambda s: s["sidebar_collapsed"] != collapsed,
        "sidebar toggle",
    )


async def select_live_mode(page):
    bounds = (await state(page))["session_mode_bounds"]
    # Segmented tabs are content-sized at the leading edge, not half-width.
    await page.mouse.click(bounds["x"] + 28,
                           bounds["y"] + bounds["height"] / 2)


async def click_picker(page, y):
    bounds = (await state(page))["session_mode_bounds"]
    await page.mouse.click(bounds["x"] + bounds["width"] - 16, y)


async def check_sidebar_session(page):
    """Sidebar placement, short-window reachability and collapse preserve history."""
    before = await state(page)
    for width, height in [(1400, 1000), (1000, 680), (1000, 500)]:
        await page.set_viewport_size({"width": width, "height": height})
        await page.wait_for_timeout(250)
        current = await state(page)
        panel = current["session_panel_bounds"]
        actions = current["session_action_bounds"]
        assert panel["x"] >= 0 and panel["x"] + panel["width"] <= current["sidebar_width"], current
        assert panel["height"] <= height * .45 + 1, panel
        assert panel["y"] + panel["height"] <= height, panel
        assert actions["y"] >= panel["y"], actions
        assert actions["x"] >= panel["x"], current
        assert actions["x"] + actions["width"] <= panel["x"] + panel["width"], current
        assert actions["y"] + actions["height"] <= height, actions
    await toggle_sidebar(page)
    collapsed = await state(page)
    assert collapsed["sidebar_collapsed"], collapsed
    bounds = collapsed["session_toggle_bounds"]
    await page.mouse.click(bounds["x"] + bounds["width"] / 2,
                           bounds["y"] + bounds["height"] / 2)
    await wait_state(page, lambda s: s["session_popover_open"], "collapsed session popover")
    await page.keyboard.press("Escape")
    await wait_state(page, lambda s: not s["session_popover_open"], "dismissed session popover")
    # Leave the trigger, then reach it again through keyboard navigation.
    await page.keyboard.press("Tab")
    for _ in range(64):
        if (await state(page))["session_toggle_focused"]:
            break
        await page.keyboard.press("Tab")
        await page.wait_for_timeout(30)
    assert (await state(page))["session_toggle_focused"], "Session trigger must be keyboard reachable"
    await page.keyboard.press("Enter")
    await wait_state(page, lambda s: s["session_popover_open"], "keyboard session popover")
    await page.keyboard.press("Escape")
    await wait_state(page, lambda s: not s["session_popover_open"], "keyboard popover dismissal")
    assert (await state(page))["session_toggle_focused"], "Keyboard dismissal restores trigger focus"
    await toggle_sidebar(page)
    await page.set_viewport_size({"width": 1400, "height": 1000})
    after = await state(page)
    assert after["session_id"] == before["session_id"], after
    assert after["history_origin"] == before["history_origin"], after
    assert after["history_bytes"] == before["history_bytes"], after
    assert after["requested_view"] == before["requested_view"], after


async def check_sidebar_live(page, live, base):
    """Focused coverage of session placement without exercising every form field."""
    with tempfile.TemporaryDirectory(dir=ROOT / "target", prefix="sidebar-smoke-") as temp:
        build = make_build(Path(temp)).relative_to(ROOT).as_posix()
        async with ClientSession() as client:
            response = await client.post(base + "/api/session/live", json={"build_dir": build})
            assert response.status == 202, await response.text()
        await wait_state(page, lambda s: s["frames_received"] >= 300, "sidebar history populated")
        await check_sidebar_session(page)
        assert live.calls == 1, "Sidebar layout must not reopen acquisition."
        await click_header(page)
        await wait_state(page, lambda s: s["dialog_open"], "sidebar Configure")
        await page.keyboard.press("Escape")
        await wait_state(page, lambda s: not s["dialog_open"], "sidebar configuration dismissed")
        await click_header(page, primary=True)
        await wait_state(page, lambda s: s["status"] == "stopped" and s["can_reconnect"],
                         "sidebar Disconnect")
        await click_header(page, primary=True)
        await wait_state(page, lambda s: s["status"] == "reading" and s["frames_received"] > 0,
                         "sidebar Reconnect")
        assert live.calls == 2, "Only explicit Reconnect should reopen acquisition."


async def check_configuration(page, screenshot=None):
    """Exercise the header/dialog, not only the exported command functions."""
    await page.set_viewport_size({"width": 1400, "height": 1000})
    with tempfile.TemporaryDirectory(dir=ROOT / "target", prefix="session-smoke-") as temp:
        build = Path(temp) / "build"
        zephyr = build / "zephyr"
        zephyr.mkdir(parents=True)
        shutil.copyfile(ROOT / ELF, zephyr / "zephyr.elf")
        (zephyr / ".config").write_text('CONFIG_BOARD_TARGET="nrf5340dk/nrf5340/cpuapp"\n')
        (zephyr / "runners.yaml").write_text(yaml.safe_dump({
            "flash-runner": "jlink", "runners": ["jlink"],
            "config": {"elf_file": "zephyr.elf"},
            "args": {"jlink": ["--device=nRF5340_xxAA", "--speed=4000"]},
        }))
        (build / "CMakeCache.txt").write_text(f"APPLICATION_SOURCE_DIR:PATH={ROOT}\n")
        label = build.relative_to(ROOT).as_posix()
        before = await state(page)

        async def configure_build():
            await click_header(page)
            await wait_state(page, lambda s: s["dialog_open"], "session dialog")
            await page.wait_for_timeout(250)  # Dialog entrance/focus must settle.
            await fill_canvas_input(page, 191, label)
            await fill_canvas_input(page, 325, "")
            await page.mouse.click(700, 435)
            return await wait_state(page, lambda s: s["build_context"], "resolved build preview")

        preview = await configure_build()
        assert preview["build_context"]["board"] == "nrf5340dk/nrf5340/cpuapp", preview
        assert preview["session_id"] == before["session_id"], preview
        assert preview["context"] is None, preview
        await page.keyboard.press("Escape")
        cancelled = await wait_state(page, lambda s: not s["dialog_open"], "cancelled draft")
        assert cancelled["context"] is None, cancelled
        assert cancelled["session_id"] == before["session_id"], cancelled

        await configure_build()
        # Start from Recording. Tab through its browse icon, Resolve and Cancel to Open.
        await page.mouse.click(650, 392)
        await page.wait_for_timeout(100)
        for _ in range(4):
            await page.keyboard.press("Tab")
            await page.wait_for_timeout(100)  # Focus dispatch updates on the next GPUI frame.
        await page.keyboard.press("Enter")
        await wait_state(page, lambda s: not s["dialog_open"], "accepted replay configuration")
        opened = await wait_state(page, lambda s: s["context"] is not None and s["status"] == "ended",
                                  "replay with build context")
        assert opened["frames_received"] == 20, opened
        assert opened["context"]["board"].split("/")[0] in opened["session_labels"][1], opened
        assert opened["session_id"] != before["session_id"], opened
        assert "samples/s" not in " ".join(opened["session_labels"]), opened

        # A preview can finish after the draft is dismissed: never apply it.
        requested = asyncio.Event()
        release = asyncio.Event()

        async def delayed_resolution(route):
            requested.set()
            await release.wait()
            await route.continue_()

        await page.route("**/api/build/resolve", delayed_resolution)
        try:
            await click_header(page)
            await wait_state(page, lambda s: s["dialog_open"], "reopened dialog")
            await page.wait_for_timeout(250)
            await select_live_mode(page)
            await wait_state(page, lambda s: s["draft_mode"] == "live", "live configuration preview")
            await asyncio.wait_for(requested.wait(), timeout=5)
            await page.keyboard.press("Escape")
            await wait_state(page, lambda s: not s["dialog_open"], "dismissed pending preview")
            async with page.expect_response("**/api/build/resolve") as response:
                release.set()
            await (await response.value).json()
            await page.wait_for_timeout(100)
            after = await state(page)
            assert after["build_context"] is None, after
            assert after["context"] == opened["context"], after
            assert after["session_id"] == opened["session_id"], after
        finally:
            release.set()
            await page.unroute("**/api/build/resolve", delayed_resolution)

        # Reload reconstructs the header from the shared session snapshot.
        await page.reload()
        await page.wait_for_selector('html[data-zview-ready="true"]', timeout=90000)
        restored = await wait_state(page, lambda s: s["connected"], "context snapshot after reload")
        assert restored["context"] == opened["context"], restored
        await toggle_sidebar(page)
        assert (await state(page))["sidebar_collapsed"]
        await click_header(page)
        await wait_state(page, lambda s: s["dialog_open"], "header with collapsed sidebar")
        await page.keyboard.press("Escape")
        await wait_state(page, lambda s: not s["dialog_open"], "collapsed dialog dismissal")
        await toggle_sidebar(page)
        assert not (await state(page))["sidebar_collapsed"]
        await page.set_viewport_size({"width": 1000, "height": 680})
        await click_header(page)
        await wait_state(page, lambda s: s["dialog_open"], "minimum-viewport configuration")
        await page.wait_for_timeout(250)
        await select_live_mode(page)
        await wait_state(page, lambda s: s["draft_mode"] == "live", "minimum-viewport live preview")
        await page.wait_for_timeout(100)
        if screenshot:
            await page.screenshot(
                path=str(Path(screenshot).with_name("phase2-session-dialog.png")),
                full_page=True,
            )
        await page.keyboard.press("Escape")
        await wait_state(page, lambda s: not s["dialog_open"], "minimum-viewport dismissal")
        await page.set_viewport_size({"width": 1400, "height": 1000})


async def check_file_pickers(page, dialogs):
    before = await state(page)
    await click_header(page)
    await wait_state(page, lambda s: s["dialog_open"], "picker configuration")
    await page.wait_for_timeout(250)

    # Actual tab component: arrow keys activate its adjacent mode.
    await select_live_mode(page)
    await wait_state(page, lambda s: s["draft_mode"] == "live", "Live tab")
    await page.wait_for_timeout(100)
    await page.keyboard.press("ArrowRight")
    await wait_state(page, lambda s: s["draft_mode"] == "replay", "keyboard Replay tab")
    await fill_canvas_input(page, 191, "")

    async def browse(y, kind, status=200):
        count = len(dialogs.requests)
        async with page.expect_response("**/api/files/choose") as response:
            await click_picker(page, y)
        response = await response.value
        assert response.status == status
        await response.json()
        chosen = await wait_state(page, lambda s: s["native_picker"] is None, f"{kind} native result")
        assert len(dialogs.requests) == count + 1
        assert dialogs.requests[-1][0] == kind
        return chosen

    # Native-dialog implementation is scripted here; the Rust/HTTP path is real.
    dialogs.result = str(ROOT / "test-data/tests/fixtures")
    chosen = await browse(191, "directory")
    assert chosen["draft_request"]["build_dir"] == "test-data/tests/fixtures", chosen
    assert dialogs.requests[-1][1] == ROOT
    assert chosen["session_id"] == before["session_id"], chosen

    # A populated file field reopens at its parent; selection uses the Rust draft path.
    await fill_canvas_input(page, 325, "test-data/tests/fixtures/unselected.elf")
    dialogs.result = str(ROOT / ELF)
    chosen = await browse(325, "elf")
    assert dialogs.requests[-1][1] == ROOT / "test-data/tests/fixtures"
    assert chosen["draft_request"]["elf"] == ELF, chosen

    await fill_canvas_input(page, 392, "test-data/tests/fixtures/recordings/unselected.ndjson.gz")
    dialogs.result = str(ROOT / RECORDING)
    chosen = await browse(392, "recording")
    assert chosen["draft_recording"] == RECORDING, chosen

    dialogs.result = None
    cancelled = await browse(325, "elf")
    assert cancelled["draft_request"]["elf"] == ELF, cancelled
    assert cancelled["draft_error"] is None, cancelled
    dialogs.result = str(ROOT.parent)
    rejected = await browse(325, "elf", status=400)
    assert rejected["draft_request"]["elf"] == ELF, rejected
    assert rejected["draft_error"], rejected

    dialogs.entered.clear()
    dialogs.wait = asyncio.Event()
    dialogs.result = str(ROOT / ELF)
    try:
        await click_picker(page, 325)
        await asyncio.wait_for(dialogs.entered.wait(), timeout=5)
        await fill_canvas_input(page, 325, "manually-changed.elf")
        async with page.expect_response("**/api/files/choose") as response:
            dialogs.wait.set()
        await (await response.value).json()
        edited = await wait_state(page, lambda s: s["native_picker"] is None, "native result after edit")
        assert edited["draft_request"]["elf"] == "manually-changed.elf", edited
    finally:
        dialogs.wait.set()
        dialogs.wait = None

    # Closing the draft while the OS dialog is open invalidates its eventual result.
    dialogs.entered.clear()
    dialogs.wait = asyncio.Event()
    dialogs.result = str(ROOT / ELF)
    try:
        await click_picker(page, 325)
        await asyncio.wait_for(dialogs.entered.wait(), timeout=5)
        await page.keyboard.press("Escape")
        await wait_state(page, lambda s: not s["dialog_open"], "closed native-picker draft")
        async with page.expect_response("**/api/files/choose") as response:
            dialogs.wait.set()
        await (await response.value).json()
        await page.wait_for_timeout(100)
        cancelled = await state(page)
        assert not cancelled["dialog_open"] and cancelled["native_picker"] is None, cancelled
    finally:
        dialogs.wait.set()
        dialogs.wait = None
    assert cancelled["session_id"] == before["session_id"], cancelled
    assert cancelled["context"] == before["context"], cancelled


class ControlledLive:
    """Simulate acquisition only; exercise the real HTTP/Rust/UI path without a probe."""

    def __init__(self):
        self.calls = 0
        self.lost = threading.Event()
        self.warning = threading.Event()
        self.resume = threading.Event()
        self.heap_test = threading.Event()

    def __call__(self, target, stop, emit, *, capture=None, period=0.1, view=None):
        self.calls += 1
        usage_window = None

        def sync_usage():
            nonlocal usage_window
            with view.lock:
                requested = view.usage_window
            if usage_window != requested:
                usage_window = requested
                emit("usage_window", {"polls": usage_window})

        sync_usage()
        if self.heap_test.is_set():
            return self.heap_worker(stop, emit, capture)
        if stop.wait(0.5):
            return "stopped", None
        groups = json.loads((ROOT / "sample.json").read_text())
        emit("capabilities", {"groups": ["threads", "heaps"], "missing": [],
                              "is_waitq_walkable": True})
        for n in range(300):
            if stop.wait(0.02):
                return "stopped", None
            if self.lost.is_set():
                return "lost", "Controlled test: probe disconnected"
            if n == 0:
                emit("status", {"status": "reading"})
            emit("frame", {"decoded_at": time.time(), "live_elapsed": n * 0.1, "groups": groups})
            if n % 20 == 0:
                emit("health", {"samples_per_second": 9.8})
        extra = 0
        last_view = view.snapshot()[0] if view is not None else None
        while not stop.wait(0.05):
            sync_usage()
            if self.lost.is_set():
                return "lost", "Controlled test: probe disconnected"
            if self.warning.is_set():
                self.warning.clear()
                emit("notice", {"message": "Controlled transient read warning"})
            changed_view = view is not None and view.snapshot()[0] != last_view
            if self.resume.is_set() or changed_view:
                last_view = view.snapshot()[0] if view is not None else None
                self.resume.clear()
                emit("frame", {"decoded_at": time.time(), "live_elapsed": 30. + extra * 0.1,
                               "groups": groups})
                extra += 1
        return "stopped", None

    def heap_worker(self, stop, emit, capture):
        """Synthetic layout only; exercise actual browser interest and wire state."""
        emit("status", {"status": "reading"})
        n = 0
        while not stop.wait(0.1):
            groups = json.loads((ROOT / "sample.json").read_text())
            original = groups["heaps"][0]
            groups["heaps"].append({
                **original, "address": original["address"] + 4096, "name": "other_heap",
            })
            request = capture.due()
            for heap in groups["heaps"]:
                heap["chunks"] = None
            heap = next((heap for heap in groups["heaps"] if request is not None
                         and (heap["address"], heap["name"]) == request[1]), None)
            chunks = [{"size": 32, "used": True}, {"size": 64, "used": False}]
            if heap is not None:
                heap["chunks"] = chunks
            at = time.time()
            emit("frame", {"decoded_at": at, "live_elapsed": n * .1, "groups": groups})
            if heap is not None and capture.accepts(request):
                emit("heap_map", {
                    "generation": request[0], "heap": {"address": heap["address"], "name": heap["name"]},
                    "chunks": chunks, "captured_at": at, "live_elapsed": n * .1, "error": None,
                })
                capture.completed(request)
            n += 1
        return "stopped", None


async def check_live(page, live, screenshot, sockets, dialogs):
    await page.set_viewport_size({"width": 1400, "height": 1000})
    with tempfile.TemporaryDirectory(dir=ROOT / "target", prefix="live-smoke-") as temp:
        root = Path(temp)
        build = make_build(root)
        runners = build / "zephyr/runners.yaml"
        metadata = yaml.safe_load(runners.read_text())
        metadata["flash-runner"] = "nrfutil"
        metadata["runners"].insert(0, "nrfutil")
        runners.write_text(yaml.safe_dump(metadata))
        label = (root / "build").relative_to(ROOT).as_posix()
        await click_header(page)
        await wait_state(page, lambda s: s["dialog_open"], "live session dialog")
        await page.wait_for_timeout(250)
        await select_live_mode(page)
        await wait_state(page, lambda s: s["draft_mode"] == "live", "Live tab")
        dialogs.result = str(build)
        async with page.expect_response("**/api/files/choose") as response:
            await click_picker(page, 191)
        assert (await response.value).status == 200
        preview = await wait_state(page, lambda s: s["build_context"] and
                                   s["build_context"]["build_dir"] == label,
                                   "automatic build-folder-only detection")
        assert preview["draft_request"] == {
            "mode": "live", "build_dir": label, "domain": "app",
            "elf": f"{label}/zephyr/zephyr.elf", "runner": "jlink",
            "target": "nRF5340_xxAA", "frequency": 4000000,
        }, preview
        assert preview["build_context"]["runner"] == "jlink", preview
        assert preview["build_context"]["target"] == "nRF5340_xxAA", preview
        assert preview["build_context"]["frequency"] == 4000000, preview
        assert not preview["draft_advanced"], preview
        # Expanding/collapsing affects visibility, not submitted values.
        await page.mouse.click(700, 235)
        await wait_state(page, lambda s: s["draft_advanced"], "Advanced settings expanded")
        await fill_canvas_input(page, 437, "pyocd")
        edited = await wait_state(page, lambda s: s["draft_request"].get("runner") == "pyocd"
                                  and s["build_context"] is not None,
                         "explicit runner override")
        assert edited["draft_request"]["target"] == "nrf5340_xxaa", edited
        await page.mouse.click(700, 235)
        collapsed = await wait_state(page, lambda s: not s["draft_advanced"], "collapse overrides")
        assert collapsed["draft_request"] == edited["draft_request"], collapsed
        await page.mouse.click(700, 235)
        reopened = await wait_state(page, lambda s: s["draft_advanced"], "reopen Advanced")
        assert reopened["draft_request"] == edited["draft_request"], reopened
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("advanced-settings-expanded.png")),
                                  full_page=True)
        async def refresh_settings():
            # Refresh is inside Advanced, after the probe-clock input.
            await page.mouse.click(650, 571)
            await page.wait_for_timeout(100)
            await page.keyboard.press("Tab")
            await page.keyboard.press("Space")

        # Refresh must resolve without overrides, then replace the actual inputs.
        async with page.expect_request("**/api/build/resolve") as refresh:
            await refresh_settings()
        assert (await refresh.value).post_data_json == {"mode": "live", "build_dir": label}
        await wait_state(page, lambda s: s["build_context"] is not None
                         and s["draft_request"] == preview["draft_request"], "refresh replaces edits")
        # Failed refresh keeps the current fields.
        async def failed_resolution(route):
            await route.fulfill(status=400, json={"error": "test detection failure"})
        await page.route("**/api/build/resolve", failed_resolution)
        try:
            await refresh_settings()
            failed = await wait_state(page, lambda s: s["draft_error"] == "test detection failure",
                                      "failed refresh")
            assert failed["draft_request"] == preview["draft_request"], failed
        finally:
            await page.unroute("**/api/build/resolve", failed_resolution)
        await page.mouse.click(700, 235)
        await wait_state(page, lambda s: not s["draft_advanced"], "collapse after refresh")
        other = make_build(root, name="other").relative_to(ROOT).as_posix()
        await fill_canvas_input(page, 191, other)
        await wait_state(page, lambda s: not s["draft_advanced"] and
                         s["draft_request"].get("runner") == "jlink" and
                         s["draft_request"].get("elf") == f"{other}/zephyr/zephyr.elf" and
                         s["build_context"] is not None, "new build detects fresh values")
        await page.mouse.click(700, 235)
        restored = await wait_state(page, lambda s: s["draft_advanced"], "new build overrides")
        assert restored["draft_request"]["build_dir"] == other, restored
        await page.mouse.click(700, 235)
        await fill_canvas_input(page, 191, label)
        await wait_state(page, lambda s: not s["draft_advanced"] and
                         s["draft_request"] == preview["draft_request"] and
                         s["build_context"] is not None,
                         "automatic settings restored")

        # A -> B -> A: equality of the request alone must not accept the first
        # response. Hold it, resolve the latest A, then return stale A metadata.
        requested, release = asyncio.Event(), asyncio.Event()

        async def stale_resolution(route):
            if requested.is_set():
                await route.continue_()
                return
            requested.set()
            response = await route.fetch()
            data = await response.json()
            data["target"] = "stale-target"
            await release.wait()
            await route.fulfill(response=response, json=data)

        await page.route("**/api/build/resolve", stale_resolution)
        try:
            await page.mouse.click(700, 235)
            await wait_state(page, lambda s: s["draft_advanced"], "expand before refresh")
            await refresh_settings()
            await asyncio.wait_for(requested.wait(), timeout=5)
            await fill_canvas_input(page, 191, other)
            await wait_state(page, lambda s: s["build_context"] and
                             s["build_context"]["build_dir"] == other, "intervening build")
            await fill_canvas_input(page, 191, label)
            await wait_state(page, lambda s: s["build_context"] and
                             s["build_context"]["build_dir"] == label, "latest build preview")
            async with page.expect_response("**/api/build/resolve") as response:
                release.set()
            await (await response.value).json()
            await page.wait_for_timeout(200)
            current = await state(page)
            assert current["build_context"]["target"] == "nRF5340_xxAA", current
            assert current["draft_error"] is None, current
        finally:
            release.set()
            await page.unroute("**/api/build/resolve", stale_resolution)
        await page.mouse.click(650, 191)
        await page.wait_for_timeout(100)
        # Confirm through the same dialog action as Connect.
        await page.keyboard.press("Enter")
        await wait_state(page, lambda s: s["mode"] == "live" and s["indicator"] == "busy",
                         "connecting spinner")
        opened = await wait_state(page, lambda s: s["mode"] == "live" and s["status"] == "reading",
                                  "Live Connect", timeout=30)
        assert not opened["dialog_open"], opened
        assert opened["session_labels"][0] == "LIVE · Polling", opened
        assert "J-Link" in opened["session_labels"][1], opened
        assert "Last sample" not in " ".join(opened["session_labels"]), opened
        full = await wait_state(page, lambda s: s["frames_received"] == 300,
                                "time-based compressed live history", timeout=40)
        assert full["buffered_frames"] == 1, full
        assert 25 < full["history_seconds"] <= 29.9, full
        assert abs(full["history_seconds"] - (full["live_elapsed"] - full["history_origin"])) < 1e-9, full
        assert full["history_changes"] < 30, full
        assert full["history_bytes"] < 64 * 1024, full
        assert abs(full["live_elapsed"] - 29.9) < 1e-9, full
        assert full["samples_per_second"] == 9.8, full
        assert full["visible_threads"] == 2, full
        assert live.calls == 1
        assert full["indicator"] == "healthy", full
        assert full["status_shimmer"], full
        # The host forwards live media changes without restarting acquisition.
        # Check freshness-dependent animation before the slower sidebar checks:
        # the controlled worker pauses after frame 300, so its status expires.
        await page.emulate_media(reduced_motion="reduce")
        reduced = await wait_state(page, lambda s: s["reduced_motion"], "browser reduced motion")
        assert not reduced["status_shimmer"], reduced
        assert reduced["indicator"] == "healthy", reduced
        await page.emulate_media(reduced_motion="no-preference")
        await wait_state(page, lambda s: s["status_shimmer"], "Live shimmer restored")
        await check_sidebar_session(page)
        assert "window_slider_bounds" not in full, "The redundant footer slider was removed."
        # The actual GPUI wheel/fit controls only change the viewport. Repeated
        # source readings and zooming must not allocate or discard chart history.
        await page.mouse.move(430, 330)
        await page.mouse.wheel(0, -300)
        await page.wait_for_timeout(150)
        zoomed = await state(page)
        assert zoomed["history_seconds"] == full["history_seconds"], zoomed
        assert zoomed["history_changes"] == full["history_changes"], zoomed
        assert zoomed["history_bytes"] == full["history_bytes"], zoomed
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("live-history-zoom.png")),
                                  full_page=True)
        await page.mouse.click(592, 289)  # Fit retained history.
        await page.mouse.click(564, 289)  # Reset icon beside Fit history.
        assert full["usage_window"] == 16, full
        # Real Kit NumberInput edits are drafts until Enter/blur. Buttons and
        # arrow keys step by one; the Core arithmetic is tested independently.
        async def edit_usage(value, *, enter=True):
            bounds = (await state(page))["usage_input_bounds"]
            assert bounds["width"] > 0 and bounds["height"] > 0, bounds
            await page.mouse.click(bounds["x"] + bounds["width"] / 2,
                                   bounds["y"] + bounds["height"] / 2)
            await page.wait_for_timeout(150)
            await page.keyboard.press("Control+A")
            await page.keyboard.press("Backspace")
            await page.keyboard.type(value, delay=20)
            if enter:
                await page.keyboard.press("Enter")
        await edit_usage("17", enter=False)
        assert (await state(page))["usage_window"] == 16, "Typing must not submit intermediate digits."
        await page.keyboard.press("Enter")
        await wait_state(page, lambda s: s["usage_window"] == 17, "direct integer CPU window")
        await page.wait_for_timeout(150)
        await page.keyboard.press("ArrowUp")
        await wait_state(page, lambda s: s["usage_window"] == 32, "CPU number-input keyboard step up")
        await page.wait_for_timeout(150)
        await page.keyboard.press("ArrowDown")
        await wait_state(page, lambda s: s["usage_window"] == 16, "CPU number-input keyboard step down")
        bounds = (await state(page))["usage_input_bounds"]
        for x, expected in ((bounds["x"] + bounds["width"] + 12, 32), (bounds["x"] - 12, 16)):
            await page.wait_for_timeout(150)
            await page.mouse.click(x, bounds["y"] + bounds["height"] / 2)
            await wait_state(page, lambda s: s["usage_window"] == expected, "number-input step button")
        for typed, expected in (("64", 64), ("1", 1), ("65", 64), ("0", 1), ("-1", 1), ("", 1), ("1.5", 1)):
            await edit_usage(typed)
            await wait_state(page, lambda s: s["usage_window"] == expected and
                             s["usage_input"] == str(expected), f"CPU input normalization {typed!r}")
        await edit_usage("23", enter=False)
        await page.keyboard.press("Tab")
        await wait_state(page, lambda s: s["usage_window"] == 23, "CPU input commits on blur")
        await edit_usage("")
        await wait_state(page, lambda s: s["usage_window"] == 23 and s["usage_input"] == "23",
                         "empty input restores applied value")
        async def reject_usage(route):
            await route.fulfill(status=400, json={"error": "Controlled CPU-setting failure"})
        await page.route("**/api/session/usage-window", reject_usage, times=1)
        await edit_usage("19")
        await wait_state(page, lambda s: s["error"] == "Controlled CPU-setting failure"
                         and s["usage_input"] == "23", "failed edit restores applied value")
        await edit_usage("32")
        await wait_state(page, lambda s: s["usage_window"] == 32 and s["error"] is None,
                         "number-input recovery after request error")
        assert live.calls == 1, "CPU averaging must not reopen the probe."
        await page.mouse.click(400, 634)
        await wait_state(page, lambda s: s["screen"].startswith("Thread(")
                         and not s["view_loading"], "thread detail with averaging controls")
        await page.set_viewport_size({"width": 1000, "height": 680})
        await page.wait_for_timeout(250)
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("cpu-averaging-thread-min.png")),
                                  full_page=True)
        await page.set_viewport_size({"width": 1400, "height": 1000})
        await page.mouse.click(100, 180)
        await wait_state(page, lambda s: s["screen"] == "KernelObjects" and not s["view_loading"],
                         "kernel objects with header controls")
        await click_header(page)
        await wait_state(page, lambda s: s["dialog_open"], "header configuration from kernel objects")
        await page.keyboard.press("Escape")
        await wait_state(page, lambda s: not s["dialog_open"], "kernel configuration dismissal")
        await page.mouse.click(100, 147)
        returned = await wait_state(page, lambda s: s["screen"] == "Overview" and not s["view_loading"], "return to Overview")
        assert returned["history_origin"] > full["history_origin"], returned
        assert returned["history_seconds"] < 1, returned
        before_warning = await state(page)
        live.warning.set()
        warning = await wait_state(page, lambda s: s["indicator"] == "warning", "amber read warning")
        assert not warning["status_shimmer"], warning
        await page.emulate_media(reduced_motion="reduce")
        await page.reload()
        await page.wait_for_selector('html[data-zview-ready="true"]', timeout=90000)
        await wait_state(page, lambda s: s["indicator"] == "warning",
                         "warning preserved by the shared snapshot after reload")
        live.resume.set()
        resumed = await wait_state(page, lambda s: s["frames_received"] > before_warning["frames_received"]
                                   and s["indicator"] == "healthy", "green after a valid frame")
        assert resumed["reduced_motion"] and not resumed["status_shimmer"], resumed
        assert 0 <= resumed["history_percent"] <= 100, resumed
        assert "/ 16.0 MiB" in resumed["history_memory_label"], resumed
        assert "/ 300 s retained" in resumed["history_retention_label"], resumed
        await page.emulate_media(reduced_motion="no-preference")
        await wait_state(page, lambda s: s["status_shimmer"], "shimmer after initial reduced motion")
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("phase3-live-polling.png")),
                                  full_page=True)
            await page.screenshot(path=str(Path(screenshot).with_name("session-header.png")),
                                  clip={"x": 0, "y": 0, "width": 320, "height": 1000})
        waiting = await wait_state(page, lambda s: s["indicator"] == "waiting",
                                   "amber when acquisition stalls despite a connected socket", timeout=8)
        assert not waiting["status_shimmer"], waiting
        await page.reload()
        await page.wait_for_selector('html[data-zview-ready="true"]', timeout=90000)
        restored = await wait_state(page, lambda s: s["mode"] == "live" and s["status"] == "reading",
                                    "live reload snapshot")
        assert restored["session_id"] == opened["session_id"], restored
        assert restored["buffered_frames"] == 1, restored
        assert restored["live_elapsed"] == resumed["live_elapsed"], restored
        assert restored["indicator"] == "waiting", restored
        assert restored["usage_window"] == 32, restored
        assert restored["usage_input"] == "32", restored
        assert live.calls == 1
        live.lost.set()
        lost = await wait_state(page, lambda s: s["status"] == "lost", "target loss")
        assert lost["samples_per_second"] is None, lost
        assert lost["cpu"] == full["cpu"], lost
        assert lost["indicator"] == "failed", lost
        assert not lost["status_shimmer"], lost
        assert lost["primary_action"] == "Reconnect", lost
        live.lost.clear()
        await click_header(page, primary=True)
        reconnected = await wait_state(page, lambda s: s["session_id"] != opened["session_id"]
                                       and s["status"] == "reading", "explicit target reconnect")
        assert live.calls == 2
        assert reconnected["buffered_frames"] == 1, reconnected
        assert reconnected["usage_window"] == 32, reconnected
        assert reconnected["usage_input"] == "32", reconnected
        await click_header(page, primary=True)
        stopped = await wait_state(page, lambda s: s["status"] == "stopped", "live disconnect")
        assert stopped["samples_per_second"] is None, stopped
        assert stopped["indicator"] == "idle", stopped
        assert not stopped["status_shimmer"], stopped
        await page.set_viewport_size({"width": 1000, "height": 680})
        await click_header(page)
        await wait_state(page, lambda s: s["dialog_open"], "minimum-size Live configuration")
        await page.wait_for_timeout(300)
        restored_draft = await state(page)
        assert not restored_draft["draft_advanced"], restored_draft
        assert restored_draft["draft_request"] == preview["draft_request"], restored_draft
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("phase3-live-dialog.png")),
                                  full_page=True)
        await page.mouse.click(500, 235)
        expanded = await wait_state(page, lambda s: s["draft_advanced"],
                                    "minimum-size Advanced expansion")
        assert expanded["draft_request"] == restored_draft["draft_request"], expanded
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("advanced-settings-min.png")),
                                  full_page=True)
        # Focus the header through the build input and its browse button.
        # Pointer activation does not automatically focus GPUI buttons.
        await page.mouse.click(500, 175)
        await page.wait_for_timeout(100)
        await page.keyboard.press("Tab")
        await page.keyboard.press("Tab")
        await page.keyboard.press("Space")
        collapsed = await wait_state(page, lambda s: not s["draft_advanced"],
                                     "keyboard collapse Advanced")
        assert collapsed["draft_request"] == expanded["draft_request"], collapsed
        await page.keyboard.press("Escape")
        await wait_state(page, lambda s: not s["dialog_open"], "Live dialog dismissal")
        await page.set_viewport_size({"width": 1400, "height": 1000})
        live.heap_test.set()
        await click_header(page, primary=True)
        await wait_state(page, lambda s: s["status"] == "reading"
                         and s["displayed_session"] == s["session_id"]
                         and s["frames_received"] >= 2, "heap test live session")
        await page.wait_for_timeout(200)
        await page.mouse.click(100, 180)
        await wait_state(page, lambda s: s["screen"] == "KernelObjects" and not s["view_loading"], "heap list")
        await page.wait_for_timeout(250)
        await page.mouse.click(480, 358)
        await wait_state(page, lambda s: s["screen"].startswith("Heap("), "actual heap row click")
        captured = await wait_state(page, lambda s: (s["displayed_heap_map_chunks"] or 0) > 0,
                                    "map displayed for the selected heap")
        assert captured["heap_capture"] is not None, captured
        assert "Last captured map" in captured["heap_map_note"], captured
        next_map = await wait_state(page, lambda s: (s["heap_map_frame"] or 0) > captured["heap_map_frame"],
                                    "next scheduled heap map")
        assert next_map["frames_received"] > captured["frames_received"], next_map
        assert next_map["sampled_view"]["screen"] == "detail", next_map
        assert next_map["sampled_groups"] == ["heaps"], next_map
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("phase4-live-heap.png")), full_page=True)
        await page.set_viewport_size({"width": 1000, "height": 680})
        await page.wait_for_timeout(300)
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("phase4-live-heap-min.png")), full_page=True)
        await page.set_viewport_size({"width": 1400, "height": 1000})
        await page.wait_for_timeout(200)
        calls = live.calls
        for socket in tuple(sockets):
            await socket.close(code=1012, message=b"Heap capture reconnect test")
        await wait_state(page, lambda s: not s["connected"], "heap stream disconnect")
        restored = await wait_state(
            page, lambda s: s["connected"] and s["heap_capture"] is not None
            and (s["displayed_heap_map_chunks"] or 0) > 0
            and s["heap_map_frame"] > next_map["heap_map_frame"],
            "heap interest restored after browser reconnect",
        )
        assert restored["session_id"] == next_map["session_id"], restored
        assert live.calls == calls, "Browser reconnect opened another acquisition worker."
        assert restored["history_origin"] == next_map["history_origin"], restored
        await page.keyboard.press("Escape")
        released = await wait_state(page, lambda s: s["screen"] == "KernelObjects" and s["heap_capture"] is None,
                                    "leaving heap details releases capture")
        await page.wait_for_timeout(1200)
        after = await state(page)
        assert after["frames_received"] > released["frames_received"], after
        assert after["heap_map_frame"] == released["heap_map_frame"], after
        await page.mouse.click(480, 358)
        reopened = await wait_state(page, lambda s: s["screen"].startswith("Heap(")
                                   and (s["displayed_heap_map_chunks"] or 0) > 0
                                   and not s["view_loading"], "new heap visit starts fresh")
        assert reopened["history_origin"] > restored["history_origin"], reopened
        assert reopened["history_seconds"] < 1, reopened
        if screenshot:
            await page.screenshot(path=str(Path(screenshot).with_name("heap-new-visit.png")), full_page=True)
        await page.keyboard.press("Escape")
        await wait_state(page, lambda s: s["heap_capture"] is None, "release reopened heap")
        await click_header(page, primary=True)
        await wait_state(page, lambda s: s["status"] == "stopped", "stop after heap capture")


async def check_view_takeover(browser, *, release_before_reply):
    """A real HTTP ownership race; release keeps a different, inactive screen."""
    live = ControlledLive()
    live.heap_test.set()
    controller = SessionController(live_reader=live)
    runner = web.AppRunner(create_app(ROOT, controller=controller))
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    base = f"http://127.0.0.1:{runner.addresses[0][1]}"
    page = await browser.new_page(viewport={"width": 1400, "height": 1000})
    try:
        with tempfile.TemporaryDirectory(dir=ROOT / "target", prefix="view-race-") as temp:
            build = make_build(Path(temp)).relative_to(ROOT).as_posix()
            async with ClientSession() as client:
                async with client.ws_connect(f"{base}/api/events?client_id=race-owner") as owner:
                    # Drain the winning tab's connection just as a browser would,
                    # so startup time cannot turn it into a disconnected slow client.
                    async def drain():
                        async for _ in owner:
                            pass
                    reader = asyncio.create_task(drain())
                    try:
                        response = await client.post(f"{base}/api/session/live",
                                                     json={"build_dir": build})
                        assert response.status == 202, await response.text()
                        sid = (await response.json())["session_id"]
                        conflict = asyncio.Event()

                        async def race(route):
                            if conflict.is_set():
                                await route.continue_()
                                return
                            # The browser already sent its Overview request before
                            # hearing of an owner. Another tab claims Objects first.
                            response = await client.post(f"{base}/api/session/view", json={
                                "session_id": sid, "client_id": "race-owner",
                                "view": {"screen": "objects", "kind": None, "query": ""},
                            })
                            assert response.status == 200, await response.text()
                            response = await route.fetch()
                            assert response.status == 409, await response.text()
                            assert (await response.json())["code"] == "view_busy"
                            if release_before_reply:
                                await owner.close()
                            await route.fulfill(response=response)
                            conflict.set()

                        await page.route("**/api/session/view", race)
                        await page.goto(base)
                        await page.wait_for_selector('html[data-zview-ready="true"]', timeout=90000)
                        await asyncio.wait_for(conflict.wait(), timeout=20)
                        if not release_before_reply:
                            # Let the losing browser consume its HTTP error before
                            # the owner closes and broadcasts the retained Objects view.
                            await wait_state(page, lambda s: s["view_error"] is not None,
                                             "losing view request completed with view_busy")
                            await owner.close()
                        recovered = await wait_state(
                            page, lambda s: s["controls_view"] and not s["view_loading"]
                            and (s["sampled_view"] or {}).get("screen") == "overview",
                            "automatic ownership recovery for a different screen",
                        )
                        assert recovered["error"] is None, recovered
                        assert recovered["view_error"] is None, recovered
                        assert live.calls == 1, "Ownership transfer reopened acquisition."
                        assert controller.session.live_view["owner"] not in (None, "race-owner")
                    finally:
                        await owner.close()
                        await reader
    finally:
        await page.close()
        await runner.cleanup()


async def check(executable_path=None, screenshot=None, sidebar_only=False):
    dialogs = ScriptedDialogs()
    live = ControlledLive()
    runner = web.AppRunner(create_app(
        ROOT, dialogs=dialogs, controller=SessionController(live_reader=live),
    ))
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    port = runner.addresses[0][1]
    try:
        async with async_playwright() as playwright:
            browser = await playwright.chromium.launch(
                executable_path=executable_path,
                args=["--enable-unsafe-webgpu", "--enable-unsafe-swiftshader"],
            )
            try:
                page = await browser.new_page(viewport={"width": 1400, "height": 1000})
                # Reloads reinitialize WASM too; allow the same startup budget
                # as the UI-ready checks instead of the 30-second default.
                page.set_default_navigation_timeout(90000)
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                await page.goto(f"http://127.0.0.1:{port}")
                await page.wait_for_selector('html[data-zview-ready="true"]', timeout=90000)
                await wait_state(page, lambda s: s["connected"], "adapter connection")
                if sidebar_only:
                    await check_sidebar_live(page, live, f"http://127.0.0.1:{port}")
                    assert not errors, errors
                    if screenshot:
                        await page.screenshot(path=screenshot, full_page=True)
                    print("Sidebar smoke passed: three viewports, mouse/keyboard popover, "
                          "history/demand preservation, Configure, Disconnect and Reconnect. "
                          "Live acquisition uses a controlled worker, not hardware.")
                    return
                await click_header(page, primary=True)
                initial = await wait_state(page, lambda s: s["dialog_open"], "header Connect opens configuration")
                assert initial["draft_mode"] == "live", initial
                await page.keyboard.press("Escape")
                await wait_state(page, lambda s: not s["dialog_open"], "initial draft cancellation")
                await open_replay(page)
                ended = await wait_state(page, lambda s: s["status"] == "ended", "replay end")
                assert not ended["status_shimmer"], ended
                assert ended["frames_received"] == 20, ended
                assert ended["buffered_frames"] == 20, ended
                assert ended["visible_threads"] == 2, ended
                assert ended["cpu"] is not None, ended
                assert ended["source"] == RECORDING, ended
                assert ended["error"] is None, ended
                assert await page.locator("canvas").count() > 0

                observer = await browser.new_page(viewport={"width": 1400, "height": 1000})
                await observer.goto(f"http://127.0.0.1:{port}")
                await observer.wait_for_selector('html[data-zview-ready="true"]', timeout=90000)
                observed = await wait_state(
                    observer, lambda s: s["status"] == "ended", "second-tab snapshot"
                )
                assert observed["frames_received"] == 20, observed
                assert observed["visible_threads"] == 2, observed
                await observer.close()

                for socket in tuple(runner.app[SOCKETS]):
                    await socket.close(code=1012, message=b"Test reconnect")
                await wait_state(page, lambda s: not s["connected"], "stream disconnect")
                restored = await wait_state(page, lambda s: s["connected"], "stream reconnect")
                assert restored["frames_received"] == 20, restored
                assert restored["buffered_frames"] == 20, restored
                await page.reload()
                await page.wait_for_selector('html[data-zview-ready="true"]', timeout=90000)
                await wait_state(page, lambda s: s["status"] == "ended", "reload snapshot")
                await open_replay(page, elf="sample.json")
                failed = await wait_state(page, lambda s: s["status"] == "failed", "bad ELF")
                assert failed["error"], failed
                await open_replay(page)
                await wait_state(page, lambda s: s["status"] == "reading", "replay restart")
                await click_header(page, primary=True)
                # "stopped" can arrive a frame before the reader is released and
                # Open is enabled again, so wait for both.
                await wait_state(page, lambda s: s["status"] == "stopped" and s["can_open"],
                                 "stop and reopen allowed")
                await open_replay(page)
                await wait_state(page, lambda s: s["status"] == "ended", "final replay")
                try:
                    await check_configuration(page, screenshot)
                    await check_file_pickers(page, dialogs)
                    await check_live(page, live, screenshot, runner.app[SOCKETS], dialogs)
                except Exception:
                    if screenshot:
                        await page.screenshot(path=screenshot, full_page=True)
                    raise
                await page.set_viewport_size({"width": 1000, "height": 680})
                assert await page.evaluate(
                    "document.documentElement.scrollWidth <= window.innerWidth"
                )
                await page.set_viewport_size({"width": 1400, "height": 1000})
                await page.evaluate("""() => new Promise(resolve =>
                    requestAnimationFrame(() => requestAnimationFrame(resolve)))""")
                if screenshot:
                    await page.screenshot(path=screenshot, full_page=True)
                    await page.screenshot(
                        path=str(Path(screenshot).with_name("session-header-stopped.png")),
                        clip={"x": 0, "y": 0, "width": 320, "height": page.viewport_size["height"]},
                    )
                assert not errors, errors
                # The ownership cases use independent servers and pages. Release
                # the long-running UI's graphics resources before starting them.
                await page.close()
                for release_before_reply in (False, True):
                    await check_view_takeover(browser, release_before_reply=release_before_reply)
                print("GPUI Rust/WASM smoke passed: typed frames into the shared UI, "
                      "two tabs, reconnect, failure/retry, stop/restart, header controls, "
                      "keyboard apply/cancel, build context, stale-preview cancellation, "
                      "reload, collapsed-sidebar controls, native-picker bridge (scripted OS boundary), "
                      "keyboard tabs, live Connect/loss/reconnect, time-based compressed live history, "
                      "green/amber/red/gray status, freshness expiry, sampling labels without sample age and minimum viewport. "
                      "Live heap detail entry, scheduled maps, interest restored after stream reconnect "
                      "without another acquisition worker, screen-scoped selection, release on navigation, "
                      "and different-screen ownership recovery before/after the busy reply. "
                      "Live acquisition uses a controlled worker, not hardware.")
            finally:
                if screenshot and not page.is_closed() and sys.exc_info()[0] is not None:
                    await page.screenshot(
                        path=str(Path(screenshot).with_name("failure.png")), full_page=True
                    )
                await browser.close()
    finally:
        await runner.cleanup()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable-path", help="Use an existing Chromium/Chrome.")
    parser.add_argument("--screenshot", help="Optional screenshot output path.")
    parser.add_argument("--sidebar-only", action="store_true",
                        help="Run focused sidebar session/layout checks with controlled Live data.")
    args = parser.parse_args()
    asyncio.run(check(args.executable_path, args.screenshot, args.sidebar_only))
