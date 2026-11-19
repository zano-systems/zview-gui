import assert from "node:assert/strict";
import test from "node:test";
import { initializeBrowser } from "./startup.mjs";

function host() {
  const calls = [];
  let motionChanged;
  const options = {
    status: { hidden: false },
    getBytes: async path => {
      calls.push(path);
      return new Uint8Array([1]);
    },
    matchMedia: query => {
      calls.push(query);
      return {
        matches: true,
        addEventListener: (_, callback) => { motionChanged = callback; },
      };
    },
    wasm: {
      default: async () => { calls.push("init"); },
      start: async () => { calls.push("start"); },
      connect_adapter: () => { calls.push("connect"); },
      set_reduced_motion: reduce => { calls.push(["motion", reduce]); },
    },
  };
  return { options, calls, motionChanged: value => motionChanged({ matches: value }) };
}

test("starts the shared UI then always connects the adapter, fetching only fonts", async () => {
  const { options, calls, motionChanged } = host();
  await initializeBrowser(options);
  assert.deepEqual(calls.filter(call => typeof call === "string" && call.startsWith("./")), [
    "./fonts/IBMPlexSans-Regular.source.ttf",
    "./fonts/JetBrainsMono-Regular.source.ttf",
  ]);
  assert.ok(calls.indexOf("start") < calls.indexOf("connect"));
  assert.equal(options.status.hidden, true);
  assert.ok(calls.some(call => Array.isArray(call) && call[0] === "motion" && call[1]));
  motionChanged(false);
  assert.deepEqual(calls.at(-1), ["motion", false]);
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
  assert.equal(calls.includes("start"), false);
  assert.equal(calls.includes("connect"), false);
});

test("reports graphics timeout without connecting", async () => {
  const { options, calls } = host();
  options.wasm.start = () => new Promise(() => {});
  await assert.rejects(initializeBrowser({ ...options, timeoutMs: 1 }), /timed out/);
  assert.equal(calls.includes("connect"), false);
});
