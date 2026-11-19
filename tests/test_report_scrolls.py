"""An exported report scrolls.

The report viewer shares one bundle with the offline animation and recording
pages, and a component's global style rules ship with the bundle whether or not
the component is drawn. The animation page's rule locking `html` and `body` to
the window took every exported report with it, from 0.1.10 until 2026-11-19:
the page stopped at the window's height in every browser.
"""

from __future__ import annotations

import re
from pathlib import Path

VIEWER = Path(__file__).resolve().parents[1] / 'src' / 'hertzforge' / 'report' / 'viewer.html'


def _rules(css: str) -> list[tuple[str, str]]:
    return [(selector.strip(), body) for selector, body in re.findall(r'([^{}]+)\{([^{}]*)\}', css)]


def test_no_rule_locks_a_report_page_to_the_window():
    html = VIEWER.read_text(encoding='utf-8')
    css = ' '.join(re.findall(r'<style[^>]*>(.*?)</style>', html, re.DOTALL))
    locking = []
    for selector, body in _rules(css):
        parts = [part.strip() for part in selector.split(',')]
        bare = [part for part in parts if part in ('html', 'body')]
        if bare and re.search(r'overflow\s*:\s*hidden', body):
            locking.append(selector)
    # the shell's own base rule locks the app's window, and the viewer's rule after it
    # takes it back; anything locking after that one wins
    scrolls = next(i for i, (selector, body) in enumerate(_rules(css))
                   if 'html' in selector and re.search(r'overflow\s*:\s*auto', body))
    later = [selector for i, (selector, body) in enumerate(_rules(css))
             if i > scrolls and selector in locking]
    assert not later, f'a rule after the viewer\'s locks the page: {later}'
    assert 'html.stage-page' in css, 'the stage pages lock the window by their own class'
