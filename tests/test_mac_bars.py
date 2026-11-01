"""The MAC as 3-D bars: one mesh, heights that are the values, 0..1.

Scene-level claims only; the window's toggle is `test_mac_bars_view`.
"""


#: a grid comfortably past OUTLINE_LIMIT, and fixed rather than
#: derived from it: a size computed from the constant under test
#: cannot falsify a change to that constant — raising the limit to
#: 10 000 asked this test for a hundred million bars and hung the
#: suite instead of failing it (2026-08-26)
DENSE = 60


def test_no_bounds_box_in_the_package_is_given_an_empty_title():
    """Every `show_bounds` call, not only this one: the waterfall, the
    paired and banded views and the MAC bars each passed '' once."""
    import ast
    from pathlib import Path

    import hertzforge

    found = []
    for path in Path(hertzforge.__file__).parent.rglob('*.py'):
        for node in ast.walk(ast.parse(path.read_text(encoding='utf-8'))):
            if (isinstance(node, ast.Call)
                    and getattr(node.func, 'attr', None) == 'show_bounds'):
                found += [f'{path.name}:{node.lineno} {k.arg}'
                          for k in node.keywords
                          if k.arg in ('xtitle', 'ytitle', 'ztitle')
                          and isinstance(k.value, ast.Constant)
                          and k.value.value == '']
    assert not found, found
