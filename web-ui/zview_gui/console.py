"""Terminal output for the adapter: levels in colour, session events as they happen."""

import os
import sys
import time

RESET = "\033[0m"
STYLES = {
    "label": "\033[1;35m",    # bold magenta "ZView GUI" prefix
    "info": "",
    "success": "\033[32m",    # green
    "warning": "\033[33m",    # yellow
    "error": "\033[1;31m",    # bold red
    "hint": "\033[2m",        # dim
    "url": "\033[1;4;36m",    # bold underlined cyan
    "value": "\033[1m",       # bold
}
PREFIX = "ZView GUI"
# Live statuses worth a line, and their level. "stopping" is followed by "stopped".
STATUSES = {
    "reading": ("success", "reading frames"),
    "lost": ("error", "connection lost"),
    "failed": ("error", "failed"),
    "stopped": ("info", "stopped"),
    "ended": ("info", "replay ended"),
}
NOTICE_REPEAT = 30.0


def wants_color(stream):
    """Colour on a terminal unless NO_COLOR is set; FORCE_COLOR forces it."""
    if os.environ.get("NO_COLOR"):
        return False
    if os.environ.get("FORCE_COLOR"):
        return True
    return hasattr(stream, "isatty") and stream.isatty()


class Console:
    """Prints ``ZView GUI: <message>`` lines, coloured by level when ``color``."""

    def __init__(self, stream=None, *, color=None, clock=time.monotonic):
        self.stream = stream
        self.color = wants_color(stream or sys.stdout) if color is None else color
        self.clock = clock
        self._status = None
        self._notice = (None, 0.0)

    def style(self, kind, text):
        code = STYLES[kind] if self.color else ""
        return f"{code}{text}{RESET}" if code else str(text)

    def say(self, level, message):
        stream = self.stream or (sys.stderr if level in {"warning", "error"} else sys.stdout)
        print(f"{self.style('label', PREFIX + ':')} {self.style(level, message)}",
              file=stream, flush=True)

    def info(self, message):
        self.say("info", message)

    def success(self, message):
        self.say("success", message)

    def warning(self, message):
        self.say("warning", message)

    def error(self, message):
        self.say("error", message)

    def hint(self, message):
        self.say("hint", message)

    def field(self, label, value):
        """``label: value`` with the value emphasised, e.g. paths at startup."""
        self.say("info", f"{label}: {self.style('value', value)}")

    def connecting(self, context):
        board = context.get("board") or "board unknown"
        target = context.get("target") or "default target"
        self.info(f"connecting to {self.style('value', board)} via "
                  f"{self.style('value', context['runner'])} ({target})")

    def serving(self, url):
        self.info(f"open {self.style('url', url)}")
        self.hint("Ctrl+C disconnects and stops the server.")

    def session_event(self, kind, data):
        """Report status changes and notices; frames and the rest stay silent."""
        if kind == "status":
            status = data.get("status")
            if status == self._status or status not in STATUSES:
                return
            self._status = status
            level, text = STATUSES[status]
            error = data.get("error")
            self.say(level, f"Live {text}: {error}" if error else f"Live {text}")
        elif kind == "notice":
            # Read faults can repeat every frame: print a repeat at most every 30 s.
            message = data.get("message", "")
            last, at = self._notice
            now = self.clock()
            if message == last and now - at < NOTICE_REPEAT:
                return
            self._notice = (message, now)
            self.warning(message)
