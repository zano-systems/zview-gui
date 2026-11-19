import * as wasm from "./pkg/zview_desktop.js";
import { initializeBrowser } from "./startup.mjs";

const status = document.querySelector("#status");

async function getBytes(path) {
  const response = await fetch(new URL(path, import.meta.url));
  if (!response.ok) throw new Error(`Cannot load ${path}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}

try {
  await initializeBrowser({
    wasm,
    getBytes,
    status,
    matchMedia: query => window.matchMedia(query),
  });
  // Host startup, not proof of a connected adapter or a completed GPU frame.
  document.documentElement.dataset.zviewReady = "true";
} catch (error) {
  console.error(error);
  status.textContent = `ZView: ${error.message ?? error}`;
  status.hidden = false;
}
