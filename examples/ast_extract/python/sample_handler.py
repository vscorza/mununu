"""
Sample Python handler for demonstrating AST-based extraction.

Models a rate-limited request handler with active/inactive state.
"""


class Handler:
    def __init__(self):
        self._active = False
        self._rate_limited = False

    def activate(self):
        if self._active:
            return
        self._active = True

    def deactivate(self):
        if not self._active:
            return
        self._active = False

    def handle_request(self):
        """Process a request. Does not check _rate_limited."""
        pass

    def set_rate_limit(self):
        self._rate_limited = True

    def clear_rate_limit(self):
        self._rate_limited = False
