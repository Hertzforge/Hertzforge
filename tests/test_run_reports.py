"""Reports from Runs (2026-10-04): `run_report` over a batch of
controller runs, from a script as a list and from the window's File
menu through one dialog, on the long-verb runner with the bar and
Cancel."""

from __future__ import annotations

import os

import pytest
from conftest import fixture_path
from test_rattlesnake_sine import _write_run

import hertzforge


def _campaign(tmp_path):
    """A sweep and a random run: two kinds, one batch."""
    return [_write_run(tmp_path / 'sweep.nc4', seconds=18.0),
            fixture_path('plate', 'random.nc4')]


def test_a_list_of_runs_is_a_batch_with_progress(tmp_path):
    out = tmp_path / 'reports'
    out.mkdir()
    ticks = []
    written = hertzforge.run_report(
        _campaign(tmp_path), str(out) + '/', marking='BATCH',
        progress=lambda done, total: ticks.append((done, total)))
    assert sorted(os.path.basename(p) for p in written) == \
        ['random.html', 'sweep.html']
    assert ticks == [(0, 2), (1, 2), (2, 2)], 'told before, and after each run'


def test_a_list_of_one_still_answers_a_list(tmp_path):
    written = hertzforge.run_report(
        [fixture_path('plate', 'random.nc4')], str(tmp_path) + '/')
    assert isinstance(written, list) and len(written) == 1


def test_a_batch_with_an_unreportable_run_writes_nothing(tmp_path):
    """Every kind is read before the first report: a modal survey
    third in line refuses the batch, not after two reports."""
    out = tmp_path / 'reports'
    out.mkdir()
    runs = [*_campaign(tmp_path), fixture_path('plate', 'modal.nc4')]
    with pytest.raises(ValueError, match='no one-call report'):
        hertzforge.run_report(runs, str(out) + '/')
    assert list(out.iterdir()) == []


def test_run_report_takes_each_runs_report_and_last(tmp_path):
    """`kinds` overrides the file's own type run by run, `last` may be
    given run by run, and a kind that is no report is refused before
    anything is written."""
    import re

    both = _write_run(tmp_path / 'both.nc4', random=True, seconds=18.0)
    out = tmp_path / 'reports'
    out.mkdir()
    with pytest.raises(ValueError, match='no report is called'):
        hertzforge.run_report([both], str(out) + '/',
                                  kinds={both: 'combined'})
    assert list(out.iterdir()) == []
    written = hertzforge.run_report(
        [both], str(out) + '/', kinds={both: 'random'}, last={both: 5.0})
    assert [os.path.basename(p) for p in written] == ['both.html'], \
        'one random report, not the two its type would get'
    with open(written[0], encoding='utf-8') as handle:
        html = handle.read()
    assert 'Random Vibration Test Report' in html
    assert float(re.search(r'over ([0-9.]+) ?s', html).group(1)) == \
        pytest.approx(5.0, abs=0.01)


def test_a_channel_with_no_reading_is_a_gap_not_a_failed_report(tmp_path):
    """A one-second random-and-sine run: some channel's RMS error has no
    number. It went into the page as NaN, which the page's JSON refuses,
    and the whole report failed to write (2026-10-06, found by the File
    menu test above). It is a null now, a labeled row with no bar."""
    import json

    from hertzforge.report import render_html

    both = _write_run(tmp_path / 'both.nc4', random=True)
    project = hertzforge.random_vibration_run(both)
    name = project.generate_report('random')
    html = render_html(project[name], project, None, object_groups=project.object_groups)
    payload = json.loads(html.split('type="application/json">')[1]
                         .split('</script>')[0])
    errors = [b for b in payload['blocks'] if b.get('kind') == 'bars'
              and b.get('ylabel', '').startswith('RMS error')]
    assert errors, 'the RMS error chart is on the page'
    assert any(value is None for chart in errors for value in chart['values'])
