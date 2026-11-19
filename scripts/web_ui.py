#!/usr/bin/env python3
"""Build the shared Rust UI and launch its required Python session adapter."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "target" / "web"
BINDGEN_VERSION = "0.2.129"  # Must match Cargo.toml and Cargo.lock.
FONT_BASE = (
    "https://raw.githubusercontent.com/longbridge/gpui-kit/"
    "67265a36e260ffe03c5aaebdcd8d654dbf892eab/crates/story-web/fonts/"
)
FONTS = {
    "IBMPlexSans-Regular.source.ttf": "975dcda37d80f038dcd143c22e33ca2d97a0cc5a929aace1c749153b0fe1afa5",
    "JetBrainsMono-Regular.source.ttf": "a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f",
    "OFL.txt": "69219391c699364b1d7901dc91d8c8d1fc03a30e58ad2d30fbba095f67d70d3d",
}


# wasm-opt (Binaryen) settings for release sites. The features are the ones
# rustc enables by default for wasm32-unknown-unknown, plus threads: GPUI's
# dependencies (parking_lot, wasm_thread) emit atomic instructions, which are
# valid on the module's unshared memory. The optimizer must keep every
# instruction the compiler emitted, or wasm-opt rejects the module.
WASM_OPT_LEVEL = "-Os"  # matches the web-release profile's opt-level "s"
WASM_OPT_FEATURES = (
    "--enable-bulk-memory", "--enable-mutable-globals", "--enable-nontrapping-float-to-int",
    "--enable-sign-ext", "--enable-reference-types", "--enable-multivalue",
    "--enable-threads",
)


def wasm_opt():
    """The wasm-opt executable: $WASM_OPT, target/tools/bin, then PATH."""
    candidates = [os.environ.get("WASM_OPT"), ROOT / "target" / "tools" / "bin" / "wasm-opt",
                  shutil.which("wasm-opt")]
    return next((Path(c) for c in candidates if c and Path(c).is_file()), None)


def optimize(wasm):
    """Shrink a release module in place with wasm-opt, if available."""
    tool = wasm_opt()
    if tool is None:
        if os.environ.get("ZVIEW_REQUIRE_WASM_OPT"):
            raise ValueError("wasm-opt is required for release sites but was not found.")
        print("wasm-opt not found; the release site is not post-optimized.", flush=True)
        return
    before = wasm.stat().st_size
    subprocess.run([str(tool), WASM_OPT_LEVEL, *WASM_OPT_FEATURES, "--strip-debug",
                    "--strip-producers", str(wasm), "-o", str(wasm)], check=True)
    print(f"wasm-opt {WASM_OPT_LEVEL}: {before:,} -> {wasm.stat().st_size:,} bytes", flush=True)


def build(release=False):
    """No npm dependencies; wasm-bindgen's web output is directly browser-loadable."""
    bindgen = ROOT / "target" / "tools" / "bin" / (
        "wasm-bindgen.exe" if os.name == "nt" else "wasm-bindgen"
    )
    if not bindgen.exists():
        found = shutil.which("wasm-bindgen")
        if not found:
            raise ValueError("Browser tools are missing. Run `just web-setup` first.")
        bindgen = Path(found)
    version = subprocess.check_output([str(bindgen), "--version"], text=True).strip()
    if version != f"wasm-bindgen {BINDGEN_VERSION}":
        raise ValueError(f"Need wasm-bindgen {BINDGEN_VERSION}; run `just web-setup`.")
    toolchain = os.environ.get("ZVIEW_WEB_TOOLCHAIN", "nightly")
    profile = "web-release" if release else "web"
    # GPUI's wasm_thread/parking_lot dependencies require nightly even though
    # this application uses the single-threaded WebPlatform.
    subprocess.run([
        "cargo", f"+{toolchain}", "rustc", "--locked", "--lib",
        "--target", "wasm32-unknown-unknown", "--profile", profile,
        "--", "-C", "link-arg=-zstack-size=8388608",
    ], cwd=ROOT, check=True)
    metadata = json.loads(subprocess.check_output(
        ["cargo", f"+{toolchain}", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT, text=True,
    ))
    wasm = Path(metadata["target_directory"]) / "wasm32-unknown-unknown" / profile / "zview_desktop.wasm"
    SITE.mkdir(parents=True, exist_ok=True)
    subprocess.run([
        str(bindgen), str(wasm), "--target", "web", "--no-typescript",
        "--out-dir", str(SITE / "pkg"),
    ], check=True)
    if release:
        optimize(SITE / "pkg" / "zview_desktop_bg.wasm")
    for name in ("index.html", "main.js", "startup.mjs"):
        shutil.copyfile(ROOT / "web" / name, SITE / name)
    fonts = SITE / "fonts"
    fonts.mkdir(exist_ok=True)
    for name, digest in FONTS.items():
        path = fonts / name
        if path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == digest:
            continue
        print(f"Downloading pinned browser font asset: {name}", flush=True)
        with urlopen(FONT_BASE + name, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f"Checksum mismatch for {name}")
        path.write_bytes(data)
    print(f"Browser site: {SITE}", flush=True)


def serve(adapter_args=(), *, python=None, skip_build=False):
    python = python or os.environ.get("ZVIEW_GUI_PYTHON") or sys.executable
    # Use the selected installed adapter, not an inferred checkout or PYTHONPATH.
    # Fail before building if that environment cannot run the required backend.
    try:
        subprocess.run([python, "-I", "-c", "import zview_gui, zview"], check=True)
    except subprocess.CalledProcessError as error:
        raise ValueError(
            f"The required Python adapter is not available in {python}. "
            "Install this checkout's adapter in that environment; see web-ui/README.md."
        ) from error
    if not skip_build:
        build()
    required = ("index.html", "main.js", "startup.mjs", "pkg/zview_desktop.js",
                "pkg/zview_desktop_bg.wasm", "fonts/OFL.txt")
    if any(not (SITE / name).is_file() for name in required):
        raise ValueError("No complete browser build found. Run `just web-build` first.")
    subprocess.run([python, "-I", "-m", "zview_gui", *adapter_args, "--site", str(SITE)],
                   cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--release", action="store_true")
    serve_parser = commands.add_parser("serve")
    serve_parser.add_argument("--python", help="Interpreter with this checkout's adapter installed.")
    serve_parser.add_argument("--skip-build", action="store_true")
    args, adapter_args = parser.parse_known_args()
    if args.command == "build" and adapter_args:
        parser.error(f"unrecognized arguments: {' '.join(adapter_args)}")
    try:
        if args.command == "build":
            build(args.release)
        else:
            serve(adapter_args, python=args.python, skip_build=args.skip_build)
    except KeyboardInterrupt:
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"ZView: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
