"""Shared base class for API-backed views.

Every fetch-and-render view in this package repeats the same shape:
spawn a threaded worker, call a ``MusicAPI`` method, deliver the result
back to the UI thread, and route exceptions through ``classify_api_error``
into a status label. ``FetchView`` collapses that boilerplate into one
generic worker plus typed access to the application.

Typed app access (``music_app``) replaces the ``getattr(self.app, ...,
None)`` guards that used to wrap every queue / player / action lookup.
The application always carries those attributes (the ``MusicAPI`` client
is lazy, so ``music_api`` is never ``None``), and the old None-guards
silently hid real bugs such as renamed action methods.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, ClassVar, TypeVar, cast

from textual import work
from textual.css.query import NoMatches
from textual.widgets import DataTable, Label, Static

from ytmusic_tui.auth import classify_api_error
from ytmusic_tui.views.guards import teardown_safe

if TYPE_CHECKING:
    from collections.abc import Callable

    from ytmusic_tui.app import YtMusicTui

# Result type for the generic fetch worker.
T = TypeVar("T")


class FetchView(Static):
    """Base for views that fetch data from the YouTube Music API.

    Subclasses set :attr:`STATUS_LABEL_ID` and call :meth:`_run_fetch`
    from their own ``load_*`` / ``refresh_*`` methods. The base provides:

    * :attr:`music_app` -- typed access to the running application, so
      ``music_api`` / ``queue_manager`` / ``player`` / ``action_*`` are
      reached directly without ``getattr`` None-guards.
    * :meth:`_run_fetch` -- a single threaded worker that fetches in the
      background, delivers the result on the UI thread, and routes errors
      to :meth:`_set_status`.
    * :meth:`_set_status` -- the one teardown-safe status updater.
    * :meth:`_cursor_row` -- a NoMatches-safe cursor-row reader for
      ``get_focused_item`` implementations.
    """

    #: The ``#id`` of this view's status :class:`~textual.widgets.Label`.
    STATUS_LABEL_ID: ClassVar[str] = ""

    @property
    def music_app(self) -> YtMusicTui:
        """The running application, typed as :class:`YtMusicTui`.

        ``self.app`` is the same object; this cast just narrows the
        ``App`` type so attribute access is statically checked. The
        ``TYPE_CHECKING`` import of ``YtMusicTui`` avoids the
        app -> views -> app import cycle at runtime.
        """
        return cast("YtMusicTui", self.app)

    @work(thread=True)
    def _run_fetch(
        self,
        fetch: Callable[[], T],
        on_success: Callable[[T], None],
        *,
        loading: str | None = None,
    ) -> None:
        """Run *fetch* in a worker thread and deliver the result.

        Args:
            fetch: A zero-argument callable that performs the (blocking)
                API call and returns the payload.
            on_success: A UI-thread callback invoked with the payload.
                It is delivered via ``call_from_thread`` and should be
                ``@teardown_safe``.
            loading: Optional status text shown before *fetch* runs (for
                example ``"Searching..."``).
        """
        if loading is not None:
            self.app.call_from_thread(self._set_status, loading)
        try:
            result = fetch()
            self.app.call_from_thread(on_success, result)
        except Exception as exc:
            # Any API/network failure is surfaced to the user via the
            # status label rather than crashing the worker.
            self.app.call_from_thread(self._set_status, classify_api_error(exc))

    @teardown_safe
    def _set_status(self, text: str) -> None:
        """Update the view's status label.

        Teardown-safe: a no-op when the view is already gone (the worker
        result can land after the view is torn down on slow machines).
        """
        self.query_one(self.STATUS_LABEL_ID, Label).update(text)

    def _cursor_row(self, table_query: str) -> int | None:
        """Return the cursor row of *table_query*, or ``None`` if absent.

        Catches only :class:`NoMatches`; callers keep their own
        ``0 <= row < len(items)`` bounds checks.
        """
        try:
            table = self.query_one(table_query, DataTable)
        except NoMatches:
            return None
        return table.cursor_row
