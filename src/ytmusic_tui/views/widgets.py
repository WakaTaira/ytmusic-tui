"""Navigation-augmented widget variants.

These thin subclasses add vim-style ``j``/``k`` row navigation to
Textual's :class:`DataTable` and :class:`ListView`, matching
spotify_player's ``SelectNextOrScrollDown`` / ``SelectPreviousOrScrollUp``
bindings.

The ``j``/``k`` bindings live on the widgets themselves (not the App) so
they only fire while a table or list is focused. Text-entry widgets such
as the search :class:`Input` and the FilterBar input keep receiving the
literal characters ``j`` and ``k``, because the keys never reach the App
binding layer while an ``Input`` has focus.

The cursor action names (``cursor_down`` / ``cursor_up``) mirror the
arrow-key bindings shipped by Textual 8.2.7 for both widgets.
"""

from __future__ import annotations

from typing import Any, ClassVar

from textual.binding import Binding, BindingType
from textual.widgets import DataTable, ListView


class NavDataTable(DataTable[Any]):
    """A :class:`DataTable` with ``j``/``k`` mapped to cursor down/up.

    Everything else (columns, rows, cursor types, events) is inherited
    unchanged. ``BINDINGS`` here is merged on top of the base bindings,
    so the arrow keys keep working.
    """

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("j", "cursor_down", "Down", show=False),
        Binding("k", "cursor_up", "Up", show=False),
    ]


class NavListView(ListView):
    """A :class:`ListView` with ``j``/``k`` mapped to cursor down/up.

    Mirrors :class:`NavDataTable`; the base ``cursor_down`` /
    ``cursor_up`` actions are the same ones bound to the arrow keys.
    """

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("j", "cursor_down", "Down", show=False),
        Binding("k", "cursor_up", "Up", show=False),
    ]
