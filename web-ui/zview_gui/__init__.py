"""ZView GUI: the browser UI for ZView and its local session adapter."""

from importlib import metadata

try:
    __version__ = metadata.version("zview-gui")
except metadata.PackageNotFoundError:  # a source checkout that is not installed
    __version__ = "0+checkout"
