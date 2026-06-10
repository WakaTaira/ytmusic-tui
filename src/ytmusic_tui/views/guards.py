"""Shared guards for view callbacks."""

from __future__ import annotations

import functools
from typing import TYPE_CHECKING

from textual.css.query import NoMatches

if TYPE_CHECKING:
    from collections.abc import Callable


def teardown_safe[**P, R](method: Callable[P, R]) -> Callable[P, R | None]:
    """No-op a UI callback when its widgets are already gone.

    Fetch workers deliver results via call_from_thread; on slow machines
    (CI) the view can be torn down before the callback lands, making
    query_one raise NoMatches and WorkerFailed sink the app/test.
    """

    @functools.wraps(method)
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> R | None:
        try:
            return method(*args, **kwargs)
        except NoMatches:
            return None

    return wrapper
