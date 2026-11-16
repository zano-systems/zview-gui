"""Launch helpers shared by ``zview-gui`` and ``west zview-gui``: no aiohttp or ZView."""

import os
from pathlib import Path

LIVE_OVERRIDES = ("domain", "elf", "runner", "target", "frequency")


def heap_period(period, requested):
    """Heap maps follow polling unless set; back-to-back polling maps back-to-back too."""
    if requested is not None:
        return requested
    return period if period > 0 else 0.001


def application_source(build_dir):
    """The application source directory recorded in ``CMakeCache.txt``, if any."""
    cache = Path(build_dir) / "CMakeCache.txt"
    if not cache.is_file():
        return None
    for line in cache.read_text(errors="replace").splitlines():
        if line.startswith("APPLICATION_SOURCE_DIR:"):
            path = Path(line.split("=", 1)[1].strip())
            return path.resolve() if path.is_dir() else None
    return None


def data_root_for(build_dir, topdir=None, extra=()):
    """
    The narrowest trusted root holding the build, its sources and ``extra`` paths.
    The west workspace is preferred when it holds them all. When their only common
    ancestor is the filesystem root, or they are on different Windows drives, only the
    build directory (and ``extra``) is used.
    """
    build = Path(build_dir).resolve()
    required = [build, *(Path(p).resolve() for p in extra)]
    source = application_source(build)
    wanted = [*required, *([source] if source else [])]
    if topdir is not None:
        top = Path(topdir).resolve()
        if all(p.is_relative_to(top) for p in wanted):
            return top
    for paths in (wanted, required):
        try:
            common = Path(os.path.commonpath([str(p) for p in paths]))
        except ValueError:  # on different Windows drives: try the required inputs alone
            continue
        if common != Path(common.anchor):
            return common if common.is_dir() else common.parent
    raise ValueError("The build and its inputs share no directory but the filesystem root.")


def live_request(root, build_dir, **overrides):
    """The adapter's Live configuration: root-relative paths, only the overrides set."""
    root = Path(root).resolve()
    request = {"build_dir": Path(build_dir).resolve().relative_to(root).as_posix()}
    for key in LIVE_OVERRIDES:
        value = overrides.get(key)
        if value is None:
            continue
        if key == "elf":
            value = Path(value).resolve().relative_to(root).as_posix()
        request[key] = value
    return request
