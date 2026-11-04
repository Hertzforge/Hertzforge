"""The repository's acknowledgments and the site's credits page name the same
work in their "Standing on" lists, in the same order. The two are written by
hand; this keeps them in step. Each check also runs on a planted difference
that must trip it, since a check that cannot fail proves nothing."""

from __future__ import annotations

import pathlib
import re
from html.parser import HTMLParser

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
ACKNOWLEDGMENTS = ROOT / 'ACKNOWLEDGMENTS.md'
CREDITS = ROOT / 'site' / 'public' / 'credits.html'


def _normalized(name: str) -> str:
    return ' '.join(name.split())


def markdown_standing_on(text: str) -> list[str]:
    """The name of each entry under the "Standing On" heading: the first
    bold run of each top-level bullet, in order."""
    section = re.split(r'^## Standing On\s*$', text, flags=re.MULTILINE | re.IGNORECASE)
    if len(section) != 2:
        raise ValueError('no single "Standing On" section')
    body = re.split(r'^## ', section[1], flags=re.MULTILINE)[0]
    names = []
    for item in re.split(r'^- ', body, flags=re.MULTILINE)[1:]:
        match = re.search(r'\*\*(.+?)\*\*', item, flags=re.DOTALL)
        if match is None:
            raise ValueError(f'an entry has no bold name: {item[:60]!r}')
        names.append(_normalized(match.group(1)))
    return names


class _StandingOn(HTMLParser):
    """Collects the first <strong> of each <li> in <ul class="standing">."""

    def __init__(self) -> None:
        super().__init__()
        self.names: list[str] = []
        self._lists = 0
        self._depth = 0
        self._item_named = True
        self._strong: list[str] | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag == 'ul':
            classes = (dict(attrs).get('class') or '').split()
            if self._lists or 'standing' in classes:
                self._lists += 1
        elif self._lists == 1 and tag == 'li':
            self._item_named = False
        elif self._lists and tag == 'strong' and not self._item_named:
            self._strong = []

    def handle_endtag(self, tag: str) -> None:
        if tag == 'ul' and self._lists:
            self._lists -= 1
        elif tag == 'strong' and self._strong is not None:
            self.names.append(_normalized(''.join(self._strong)))
            self._strong = None
            self._item_named = True
        elif tag == 'li' and self._lists == 1 and not self._item_named:
            raise ValueError('an entry has no <strong> name')

    def handle_data(self, data: str) -> None:
        if self._strong is not None:
            self._strong.append(data)


def html_standing_on(text: str) -> list[str]:
    """The name of each entry in the credits page's "Standing On" list."""
    parser = _StandingOn()
    parser.feed(text)
    parser.close()
    return parser.names


def test_the_two_lists_name_the_same_work_in_the_same_order():
    md = markdown_standing_on(ACKNOWLEDGMENTS.read_text(encoding='utf-8'))
    html = html_standing_on(CREDITS.read_text(encoding='utf-8'))
    assert md, 'ACKNOWLEDGMENTS.md lists nothing under "Standing On"'
    assert md == html


def test_a_name_missing_from_the_page_is_caught():
    md = markdown_standing_on(ACKNOWLEDGMENTS.read_text(encoding='utf-8'))
    page = CREDITS.read_text(encoding='utf-8')
    first = re.search(r'<li>.*?</li>\s*', page.split('class="standing"', 1)[1], re.DOTALL)
    assert first is not None
    planted = page.replace(first.group(0), '', 1)
    assert html_standing_on(planted) != md


def test_a_renamed_entry_is_caught():
    text = ACKNOWLEDGMENTS.read_text(encoding='utf-8')
    html = html_standing_on(CREDITS.read_text(encoding='utf-8'))
    planted = text.replace('**Bendat and Piersol**', '**Bendat**', 1)
    assert planted != text
    assert markdown_standing_on(planted) != html


def test_the_parsers_read_a_small_case():
    md = ('# A\n\n## Standing On\n\nIntro.\n\n- **One**, a.\n- The **Two  Words**\n'
          '  convention.\n\n## Next\n\n- **Not this**\n')
    page = ('<ul><li><strong>Elsewhere</strong></li></ul>'
            '<ul class="standing"><li><strong>One</strong>, a <strong>b</strong>.</li>'
            '<li>The <strong>Two\n Words</strong> convention.</li></ul>')
    assert markdown_standing_on(md) == ['One', 'Two Words']
    assert html_standing_on(page) == ['One', 'Two Words']
