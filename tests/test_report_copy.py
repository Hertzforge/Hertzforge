"""Copying a report figure to the clipboard.

Every figure on the report page carries a copy button in its corner,
shown on hover the way a chat's code blocks show theirs (Brandon,
2026-09-24), so a plot goes into a document or an email without a
screenshot. The canvas is copied as drawn, at its own resolution.

One button, two ways off the page, and both are tested here: inside
the application the PNG crosses the web channel to `_Bridge.copy_image`,
which owns the real clipboard; the exported file, open in a browser,
uses the browser's clipboard API. The button must not print, and in
edit mode its click must not select the block it sits on.
"""

from __future__ import annotations

#: the page is one inline script: complete means it ran to the end or
#: threw, and a thrown page has no copy buttons to find
READY = ("if (document.readyState !== 'complete'"
         "    || !document.getElementById('data')"
         "    || !document.querySelector('.copy')) return null;")
