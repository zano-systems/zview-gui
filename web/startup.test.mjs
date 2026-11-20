import assert from "node:assert/strict";
import test from "node:test";
import { initializeBrowser } from "./startup.mjs";

function host() {
  const calls = [];
  let motionChanged;
  const uiFont = new Uint8Array([1]);
  const monoFont = new Uint8Array([2]);
  const options = {
    status: { hidden: false },
    getBytes: async path => {
      calls.push(path);
      return path.includes("IBMPlexSans") ? uiFont : monoFont;
    },
    matchMedia: query => {
      calls.push(query);
      return {
        matches: false,
        addEventListener: (event, callback) => {
          calls.push(["listen", event]);
          motionChanged = callback;
        },
      };
    },
    wasm: {
      default: async () => { calls.push("init"); },
      start: async (...fonts) => { calls.push(["start", ...fonts]); },
      connect_adapter: () => { calls.push("connect"); },
      set_reduced_motion: reduce => { calls.push(["motion", reduce]); },
    },
  };
  return { options, calls, uiFont, monoFont, motionChanged: value => motionChanged({ matches: value }) };
}

test("starts the shared UI then always connects the adapter, fetching only fonts", async () => {
  const { options, calls, motionChanged, uiFont, monoFont } = host();
  await initializeBrowser(options);
  assert.deepEqual(calls.filter(call => typeof call === "string" && call.startsWith("./")), [
    "./fonts/IBMPlexSans-Regular.source.ttf",
    "./fonts/JetBrainsMono-Regular.source.ttf",
  ]);
  assert.deepEqual(calls.slice(-5), [
    ["start", uiFont, monoFont], "(prefers-reduced-motion: reduce)",
    ["motion", false], ["listen", "change"], "connect",
  ]);
  assert.equal(options.status.hidden, true);
  motionChanged(true);
  assert.deepEqual(calls.at(-1), ["motion", true]);
  motionChanged(false);
  assert.deepEqual(calls.at(-1), ["motion", false]);
});

test("waits for WASM initialization before starting the window", async () => {
  const { options, calls } = host();
  const init = Promise.withResolvers();
  options.wasm.default = () => {
    calls.push("init");
    return init.promise;
  };
  const startup = initializeBrowser(options);
  await new Promise(resolve => setImmediate(resolve));
  const beforeInit = [...calls];
  init.resolve();
  await startup;
  assert.ok(beforeInit.includes("init"));
  assert.ok(!beforeInit.some(call => Array.isArray(call) && call[0] === "start"));
});

test("waits for the GPUI window before connecting", async () => {
  const { options, calls } = host();
  let ready;
  options.wasm.start = () => new Promise(resolve => { ready = resolve; });
  const startup = initializeBrowser(options);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(calls.includes("connect"), false);
  assert.equal(options.status.hidden, false);
  ready();
  await startup;
  assert.equal(calls.includes("connect"), true);
});

test("failed graphics initialization does not connect or fall back to data", async () => {
  const { options, calls } = host();
  options.wasm.start = async () => { throw new Error("GPU unavailable"); };
  await assert.rejects(initializeBrowser(options), /GPU unavailable/);
  assert.equal(calls.includes("connect"), false);
  assert.equal(options.status.hidden, false);
});

test("font failure does not connect or start a sample session", async () => {
  const { options, calls } = host();
  options.getBytes = async () => { throw new Error("Font unavailable"); };
  await assert.rejects(initializeBrowser(options), /Font unavailable/);
  assert.ok(!calls.some(call => Array.isArray(call) && call[0] === "start"));
  assert.equal(calls.includes("connect"), false);
});

test("reports graphics timeout without connecting", async () => {
  const { options, calls } = host();
  options.wasm.start = () => new Promise(() => {});
  await assert.rejects(initializeBrowser({ ...options, timeoutMs: 1 }), /timed out/);
  assert.equal(calls.includes("connect"), false);
});
