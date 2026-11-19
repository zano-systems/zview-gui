// Browser bootstrap only. Session state, errors and reconnects belong to Rust.
export async function initializeBrowser({
  wasm, getBytes, status, matchMedia, timeoutMs = 60000,
}) {
  const [, uiFont, monoFont] = await Promise.all([
    wasm.default(),
    getBytes("./fonts/IBMPlexSans-Regular.source.ttf"),
    getBytes("./fonts/JetBrainsMono-Regular.source.ttf"),
  ]);
  let timer;
  try {
    await Promise.race([
      wasm.start(uiFont, monoFont),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(
          "Graphics initialization timed out. Try a browser with WebGPU or WebGL2 enabled.",
        )), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }

  const motion = matchMedia("(prefers-reduced-motion: reduce)");
  wasm.set_reduced_motion(motion.matches);
  motion.addEventListener("change", event => wasm.set_reduced_motion(event.matches));
  // No sample or file import fallback. Rust reports adapter failures in the UI
  // and retries, so the startup overlay must not cover its session controls.
  status.hidden = true;
  wasm.connect_adapter();
}
