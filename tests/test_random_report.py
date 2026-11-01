"""The random vibration report, against a real run.

The template is only worth anything if it binds to what an imported
Rattlesnake run actually contains and renders every block. These build
it from `random.nc4` and check the page that comes out, rather than
checking the template's own dict.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.report import random_template
from hertzforge.report import render_html

#: the report's bar charts: RMS error and band-outside-abort, on the
#: narrowband spectra and again on the octave bands, plus the kurtosis
#: reading of the record every report carries (Brandon, 2026-08-24)
BAR_CHARTS = 5


def blocks_of(template):
    return [b for b in template.blocks]


def test_the_front_matter_is_the_modal_reports():
    """A reader has to know what was tested and where it was
    instrumented before any spectrum means anything."""
    from hertzforge.core.report import random_template

    kinds = [(b.get('kind'), b.get('caption', '')) for b in
             random_template({}).blocks]
    scenes = [c for k, c in kinds if k == 'scene']
    assert any('geometry' in c.lower() for c in scenes)
    assert any('Excitation degrees of freedom' in c for c in scenes)
    assert any('Response degrees of freedom' in c for c in scenes)
    assert any(k == 'photo' for k, _c in kinds)


def test_coherence_is_never_a_thicket_of_stacked_lines():
    """A dozen channels of coherence stacked on one 2-D axis says
    nothing. The map answered that first — frequency across, channel
    down — and the stage answers it the way the app now does, the
    channels spread in depth with each trace still readable (Brandon,
    2026-08-23). Either reading is right; flat curves are not."""
    from hertzforge.core.report import random_template

    coherence = next(b for b in random_template({}).blocks
                     if b.get('source') == '@basis:MultipleCoherence')
    assert coherence['mode'] in ('stage', 'map')
    assert coherence['mode'] != 'curves'


def test_a_binding_never_answers_with_what_a_narrower_one_claims():
    """Specification subclasses Psd, so '@basis:Psd' answered with the
    specification — and the report compared the control PSDs against
    their specification by comparing it with itself: one curve where
    there should be two, and a perfect match whatever the run had done.
    """
    from hertzforge.core.report import resolve_binding

    project = {'Spec': hertzforge.Specification(
        abscissa=np.array([10.0, 100.0]),
        ordinate=np.atleast_2d([1e-3, 1e-3]), response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'])}
    assert resolve_binding('@basis:Psd', project) is None, (
        'a specification is not the answer to a request for a PSD')
    assert resolve_binding('@basis:Specification', project) == 'Spec'

    project['Measured'] = hertzforge.Psd(
        abscissa=np.array([10.0, 100.0]),
        ordinate=np.atleast_2d([1e-3, 1e-3]), response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'])
    assert resolve_binding('@basis:Psd', project) == 'Measured'


def test_a_banded_object_answers_only_to_its_own_token():
    """An octave-band PSD is a Psd and an octave-band specification a
    Specification, and each is what the report's octave section
    compares (Brandon, 2026-09-18): the plain token never answers
    with the banded object, and the banded token never with the
    plain one."""
    from hertzforge.core.report import binding_label, resolve_binding

    f = np.linspace(10.0, 2000.0, 200)
    level = np.full((1, 200), 1e-3)
    spec = hertzforge.Specification(
        f, level, response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'], ordinate_unit=['m/s**2'],
        abort_upper=level * 4, abort_lower=level / 4)
    psd = hertzforge.Psd(f, level, response_dof=['101Z+'],
                             ordinate_dim=['acceleration**2/frequency'],
                             ordinate_unit=['m/s**2'])
    project = {'Octave Spec': spec.to_octave(6), 'Octave PSD': psd.to_octave(6),
               'Spec': spec, 'PSD': psd}
    assert resolve_binding('@basis:Psd', project) == 'PSD'
    assert resolve_binding('@basis:OctavePsd', project) == 'Octave PSD'
    assert resolve_binding('@basis:Specification', project) == 'Spec'
    assert resolve_binding('@basis:OctaveSpecification', project) == 'Octave Spec'
    del project['PSD'], project['Spec']
    assert resolve_binding('@basis:Psd', project) is None, \
        'the banded PSD is not an answer for the narrowband one'
    assert resolve_binding('@basis:Specification', project) is None
    assert binding_label('@basis:OctavePsd') == 'Basis Octave PSD (auto)'
    assert binding_label('@basis:OctaveSpecification') == \
        'Basis Octave Specification (auto)'


def test_the_script_workflow_bands_the_specification_too():
    project = hertzforge.random_vibration_run(
        fixture_path('plate', 'random.nc4'), per_octave=6)
    banded = [name for name, obj in project.items()
              if isinstance(obj, hertzforge.Specification)
              and obj.bandwidth is not None]
    assert len(banded) == 1
    assert 'to_octave' in ''.join(project.journal) or any(
        'compute_octave' in line for line in project.journal)


def test_the_comparison_is_drawn_on_the_measurements_own_axis():
    """The two rarely share one — a specification is written at
    breakpoints or on the controller's lines — so the specification is
    interpolated onto the measurement rather than the other way about."""
    from hertzforge.report import _plot_block

    spec = hertzforge.Specification(
        abscissa=np.array([10.0, 100.0, 1000.0]),
        ordinate=np.atleast_2d([1e-3, 1e-3, 1e-3]), response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'],
        abort_upper=np.atleast_2d([4e-3, 4e-3, 4e-3]),
        abort_lower=np.atleast_2d([2.5e-4, 2.5e-4, 2.5e-4]))
    lines = np.linspace(10.0, 1000.0, 400)
    # held unscaled: the response is over abort on purpose, and
    # detection would lay it back onto the specification first
    measured = hertzforge.Psd(abscissa=lines,
                         ordinate=np.atleast_2d(np.full(400, 9e-3)),
                         response_dof=['101Z+'],
                         ordinate_dim=['acceleration**2/frequency'])
    measured.scale_db = 0
    built = _plot_block(
        {'kind': 'plot', 'source': 'P', 'specification': 'S',
         'mode': 'curves'}, measured, {'S': spec, 'P': measured}, hertzforge.SI)
    assert len(built['x']) == 400, "the measurement's grid, not the spec's"
    channel = built['channels'][0]
    assert len(channel['response']) == 400
    assert len(channel['y']) == 400, 'the target, interpolated onto it'
    marked = sum(1 for v in channel['over'] if v is not None)
    assert marked > 300, 'well over abort everywhere, and marked everywhere'


def test_the_marked_lines_are_the_lines_the_table_counts():
    """One rule, asked once. The marks were computed against the raw
    limits while the response came back converted, so the comparison
    was m/s^2 against whatever the report was set to — it agreed with
    the table only in SI, and by luck."""
    from hertzforge.core.compliance import compare_all
    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    counted = dict(compare_all(spec, psds))
    for units in (hertzforge.SI, hertzforge.IN_LBF_S):
        built = _plot_block(
            {'kind': 'plot', 'source': 'P', 'specification': 'S',
             'mode': 'curves'}, psds, {'S': spec, 'P': psds}, units)
        for channel in built['channels']:
            marked = sum(1 for key in ('over', 'under')
                         for v in channel.get(key, []) if v is not None)
            assert marked == counted[channel['label']]['abort_lines'], (
                f'{channel["label"]} in {units.name}')


def test_a_density_is_drawn_flat_across_its_own_bin():
    """A polyline through the line centers draws a slope that is not in
    the data. The app steps a density; so does the report.

    And only a density. A written specification's points are
    breakpoints of a continuous curve — that is what its area is taken
    under — so a staircase is a shape it does not have. The report used
    to step it anyway, because the flag was set for a specification
    *with limits*, which meant "the zones are being drawn too" and not
    "this is a density".
    """
    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    lines = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    comparison = _plot_block(
        {'kind': 'plot', 'source': 'P', 'specification': 'S',
         'mode': 'curves'}, psds, {'S': lines, 'P': psds}, hertzforge.SI)
    assert psds.interpolation == 'bin'
    assert comparison['steps'] is True
    # the controller's target on its lines is a density per line too,
    # and steps (2026-09-19); a written breakpoint curve is the one
    # shape that does not
    assert lines.interpolation == 'bin'
    assert _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves'},
                       lines, {}, hertzforge.SI)['steps'] is True
    written = hertzforge.Specification(
        np.array([20.0, 80.0, 800.0, 2000.0]),
        np.array([[1e-3, 4e-3, 4e-3, 1e-3]]), response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'], ordinate_unit=['m/s**2'])
    assert written.interpolation == 'log_log'
    alone = _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves'},
                        written, {}, hertzforge.SI)
    assert alone.get('steps', False) is False


def test_the_reference_is_gray_and_the_measurement_is_a_curve_color():
    """The app's own reading of the pair: gray behind because it is the
    reference, the first curve color in front because it is what is
    being looked at. It was the page's ink, which could not be told
    from the gray in print (Brandon, 2026-09-27)."""
    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    built = _plot_block(
        {'kind': 'plot', 'source': 'P', 'specification': 'S',
         'mode': 'curves'}, psds, {'S': spec, 'P': psds}, hertzforge.SI)
    target, measured = built['curves']
    assert target.get('gray') and not target.get('ink')
    assert measured.get('color') == 0
    assert not measured.get('ink') and not measured.get('gray')


def test_the_report_carries_both_bar_charts():
    """The table these replaced is gone rather than sitting beside
    them: a table of six channels is read, a table of sixty is
    scanned, and the same numbers as bars answer which channel is
    worst before anything is read at all."""
    from hertzforge.core.report import random_template

    blocks = random_template({}).blocks
    charts = [b for b in blocks if b.get('kind') == 'bars']
    assert [b['mode'] for b in charts] == ['error', 'lines',
                                           'error', 'lines',
                                           'kurtosis'], (
        'both readings, narrowband and then on octave bands')
    assert [b.get('octave') for b in charts] == [None] * 5, (
        'no chart bands for itself: the octave pair reads the banded objects')
    assert [(b['source'], b.get('measured')) for b in charts[2:4]] == [
        ('@basis:OctaveSpecification', '@basis:OctavePsd')] * 2, (
        'the banded requirement against the banded measurement')
    assert not any(b.get('kind') == 'compliance' for b in blocks), (
        'the comparison table is gone entirely')


def test_the_error_chart_is_in_dB_and_the_lines_chart_a_share():
    """A tolerance on the level is written in dB and read in dB. A
    share of the band has a ceiling and no floor — no amount of staying
    inside the abort limits is a fault."""
    from hertzforge.core.compliance import ERROR_DB, LINES_PERCENT
    from hertzforge.report import _bars_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    error = _bars_block({'mode': 'error'}, spec, psds, hertzforge.SI)
    assert (error['low'], error['high']) == (-ERROR_DB, ERROR_DB)
    assert error['units'].strip() == 'dB'
    lines = _bars_block({'mode': 'lines'}, spec, psds, hertzforge.SI)
    assert (lines['low'], lines['high']) == (LINES_PERCENT, None)
    assert all(v >= 0 for v in lines['values'])


def test_the_bars_are_the_numbers_the_comparison_gives():
    from hertzforge.core.compliance import channel_errors, compare_all
    from hertzforge.report import _bars_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    rows = channel_errors(compare_all(spec, psds))
    error = _bars_block({'mode': 'error'}, spec, psds, hertzforge.SI)
    assert error['labels'] == [label for label, _d, _p in rows]
    assert error['values'] == pytest.approx([d for _l, d, _p in rows],
                                            abs=1e-3)


def test_a_bar_chart_with_nothing_in_common_renders_nothing():
    """Not a chart of zeros, which would read as six channels that
    matched."""
    from hertzforge.report import _bars_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    elsewhere = hertzforge.Psd(
        abscissa=spec.abscissa, ordinate=np.ones_like(spec.ordinate.real),
        response_dof=[f'9{i}Z+' for i in range(spec.num_records)],
        ordinate_dim=spec.ordinate_dim)
    assert _bars_block({'mode': 'error'}, spec, elsewhere, hertzforge.SI) is None


def test_the_bar_charts_are_numbered_figures():
    """They were left out of the numbering, so they had no label at
    all — and the text that refers to them by name had nothing to
    point at."""
    import json
    import re

    from hertzforge.core.report import random_template

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    psds = loaded['time_data'].compute_psds()
    project = {'Specification': loaded['Random_specification'],
               'Measured': psds,
               'Octave Specification': loaded['Random_specification'].to_octave(6),
               'Octave Measured': psds.to_octave(6)}
    page = render_html(random_template(project), project,
                       unit_system=hertzforge.SI)
    payload = json.loads(re.search(
        r'<script id="data"[^>]*>(.*?)</script>', page,
        re.DOTALL).group(1))
    bars = [b for b in payload['blocks'] if b['kind'] == 'bars']
    assert len(bars) == 4, 'narrowband and octave, two readings each'
    for block in bars:
        assert block.get('label', '').startswith('Figure ')


def test_a_share_of_a_band_cannot_be_negative():
    """So its axis does not pretend it might be. An error in dB runs
    both ways and gets whatever the data needs."""
    from hertzforge.report import _bars_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    assert _bars_block({'mode': 'lines'}, spec, psds, hertzforge.SI)['floor'] == 0.0
    assert _bars_block({'mode': 'error'}, spec, psds, hertzforge.SI)['floor'] is None


def test_the_octave_section_bands_the_measurement_for_itself():
    """The block says which fraction it wants and the report does the
    integration, so a template describes the document rather than
    listing what the project must already contain."""
    from hertzforge.report import _bars_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    plain = _bars_block({'mode': 'error'}, spec, psds, hertzforge.SI)
    banded = _bars_block({'mode': 'error', 'octave': 6}, spec, psds,
                         hertzforge.SI)
    assert plain['labels'] == banded['labels'], 'the same control channels'
    # the specification is banded alongside — bands compare only with
    # the same bands (2026-09-19) — and banding conserves the area on
    # both sides, so the level each channel came out at does not move
    assert banded['values'] == pytest.approx(plain['values'], abs=0.05)


def test_the_octave_comparison_reads_the_same_channels():
    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    built = _plot_block(
        {'kind': 'plot', 'source': 'P', 'specification': 'S',
         'mode': 'curves', 'octave': 6}, psds, {'S': spec, 'P': psds},
        hertzforge.SI)
    assert built['steps'], 'bands are drawn flat across their own width'
    assert len(built['channels']) > 1
    assert len(built['x']) < len(psds.abscissa), 'fewer bands than lines'


def _pair_for_comparison():
    lines = np.linspace(5.0, 2000.0, 800)
    spec = hertzforge.Specification(
        abscissa=np.array([10.0, 100.0, 1000.0]),
        ordinate=np.atleast_2d([1e-3, 1e-3, 1e-3]), response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'],
        abort_upper=np.atleast_2d([4e-3, 4e-3, 4e-3]),
        abort_lower=np.atleast_2d([2.5e-4, 2.5e-4, 2.5e-4]))
    measured = hertzforge.Psd(abscissa=lines,
                                  ordinate=np.atleast_2d(np.full(800, 9e-3)),
                                  response_dof=['101Z+'],
                                  ordinate_dim=['acceleration**2/frequency'])
    measured.scale_db = 0
    return spec, measured


def test_the_comparison_opens_on_the_specifications_band():
    """The axis ran to the response's band, well past the requirement
    (Brandon, 2026-09-18): the figure opens on the specification's
    own band — its lines, or a banded one's outer edges — and the
    rest is a zoom away."""
    from hertzforge.report import _plot_block

    spec, measured = _pair_for_comparison()
    built = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                         'mode': 'curves'}, measured, {'S': spec, 'P': measured},
                        hertzforge.SI)
    assert built['home_x'] == [10.0, 1000.0]
    assert min(built['x']) == 5.0 and max(built['x']) == 2000.0, \
        'the measurement is still there to zoom out to'
    banded_spec = spec.to_octave(3)
    banded = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                          'mode': 'curves'}, measured.to_octave(6),
                         {'S': banded_spec, 'P': measured.to_octave(6)},
                         hertzforge.SI)
    edges = banded_spec.bin_edges()
    # an octave-band comparison reads on a log axis (2026-09-20): its
    # opening window is the outer edges, in decades
    assert banded['logx'] is True
    assert banded['home_x'] == pytest.approx(list(np.log10([edges[0], edges[-1]])))


def test_a_banded_specification_is_drawn_on_its_own_bins():
    """Its target and zones flat across each of *its* bands, beside a
    response stepped on the measurement's: the requirement used to be
    interpolated onto the measurement's grid and stepped there, a
    staircase of the measurement's making (Brandon, 2026-09-18)."""
    from hertzforge.report import _plot_block

    spec, measured = _pair_for_comparison()
    banded_spec, banded_psd = spec.to_octave(3), measured.to_octave(6)
    built = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                         'mode': 'curves'}, banded_psd,
                        {'S': banded_spec, 'P': banded_psd}, hertzforge.SI)
    target, response = built['curves']
    # in decades: an octave-band comparison reads on a log axis (2026-09-20)
    assert built['logx'] is True
    assert target['x'] == pytest.approx(list(np.log10(banded_spec.abscissa)))
    assert target['steps'] is True
    assert target['edges'] == pytest.approx(list(np.log10(banded_spec.bin_edges())))
    channel = built['channels'][0]
    assert len(channel['y']) == len(banded_spec.abscissa)
    zone = next(z for z in channel['zones'] if z['lower'] is not None)
    assert len(zone['lower']) == len(banded_spec.abscissa)
    assert len(channel['response']) == len(built['x']) == len(banded_psd.abscissa)
    assert len(channel['over']) == len(built['x']), 'the marks on the measurement'
    assert response['x'] is None and 'edges' not in response
    # and a specification on lines stays on the measurement's axis
    plain = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                         'mode': 'curves'}, measured, {'S': spec, 'P': measured},
                        hertzforge.SI)
    assert plain['curves'][0]['x'] is None
    assert len(plain['channels'][0]['y']) == len(plain['x'])


def test_a_block_may_name_its_channel():
    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    named = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                         'mode': 'curves', 'channel': '104Z+',
                         'caption': 'Control against specification — 104Z+'},
                        psds, {'S': spec, 'P': psds}, hertzforge.SI)
    assert [ch['label'] for ch in named['channels']] == ['104Z+']
    assert named['caption'] == 'Control against specification — 104Z+', \
        'no drop-down note: there is one channel'
    assert _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                        'mode': 'curves', 'channel': '999Z+'},
                       psds, {'S': spec, 'P': psds}, hertzforge.SI) is None


def test_the_template_writes_a_figure_per_control_channel(monkeypatch):
    """One figure per control channel, narrowband and on octave bands,
    rather than one figure with a drop-down (Brandon, 2026-09-18); a
    project with nothing bound yet gets the one figure that picks.
    Up to `GRID_ABOVE` channels — the plate's eight are held under it
    here; above it the figures are a grid (the next test)."""
    from hertzforge.core import report as core_report
    from hertzforge.core.report import control_channel_labels, random_template

    monkeypatch.setattr(core_report, 'GRID_ABOVE', 100)
    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    project = {'Specification': spec, 'Measured': psds,
               'Octave Specification': spec.to_octave(6),
               'Octave Measured': psds.to_octave(6)}
    labels = control_channel_labels(project, None)
    assert len(labels) == 8 and labels[0] == '101Z+'
    blocks = random_template(project).blocks
    narrow = [b for b in blocks if b.get('specification') == '@basis:Specification'
              and b.get('kind') == 'plot']
    octave = [b for b in blocks
              if b.get('specification') == '@basis:OctaveSpecification'
              and b.get('kind') == 'plot']
    assert [b['channel'] for b in narrow] == labels
    assert [b['channel'] for b in octave] == labels
    assert narrow[0]['caption'] == 'Control against specification — 101Z+'
    assert octave[-1]['caption'].endswith(f'octave bands — {labels[-1]}')
    empty = random_template({}).blocks
    lone = [b for b in empty if b.get('specification') == '@basis:Specification']
    assert len(lone) == 1 and 'channel' not in lone[0]
    import json
    import re

    page = render_html(random_template(project), project,
                       unit_system=hertzforge.SI)
    payload = json.loads(re.search(
        r'<script id="data"[^>]*>(.*?)</script>', page, re.DOTALL).group(1))
    captions = [b.get('caption', '') for b in payload['blocks']
                if b.get('kind') == 'plot']
    comparisons = [c for c in captions if c.startswith('Control against specification')]
    assert len(comparisons) == 16
    assert not [c for c in captions if c.startswith('Test specification')], (
        'the specification is drawn against a measurement, never alone'
    )
    assert not any('drop-down' in c for c in captions), \
        'no drop-down anywhere in the random report'
    lone_specs = [b for b in random_template({}).blocks
                  if b.get('source', '').endswith('Specification')
                  and b.get('kind') == 'plot' and not b.get('specification')]
    assert lone_specs == []


def test_many_control_channels_read_as_a_grid():
    """Above four control channels a sequence of figures stops reading
    (Brandon, 2026-09-19): the template writes one grid figure in each
    place — a row per node, a column per direction — and the page gets
    a cell per channel, each the figure that channel alone would have
    had. The plate's eight channels are all Z, so one column; with the
    geometry linked it is 'Global Z', without it the DOF's own letter."""
    import json
    import re

    from hertzforge.core.report import GRID_ABOVE, random_template

    assert GRID_ABOVE == 4
    project = hertzforge.random_vibration_run(
        fixture_path('plate', 'random.nc4'),
        geometry=fixture_path('plate', 'geometry.npz'), length_unit='m')
    report = random_template(project, object_groups=project.object_groups)
    grids = [b for b in report.blocks if b.get('grid')]
    assert [b['caption'] for b in grids] == [
        'Control against specification',
        'Control against specification, octave bands']
    assert not any(b.get('channel') for b in report.blocks), \
        'no per-channel figures beside the grids'

    def payload_of(project, report):
        page = render_html(report, project, object_groups=project.object_groups,
                           unit_system=hertzforge.SI)
        return json.loads(re.search(
            r'<script id="data"[^>]*>(.*?)</script>', page, re.DOTALL).group(1))

    payload = payload_of(project, report)
    drawn = [b for b in payload['blocks'] if b['kind'] == 'grid']
    assert len(drawn) == 2 and drawn[0]['caption'] == 'Control against specification'
    control = drawn[0]
    assert control['columns'] == ['Global Z']
    assert [row['label'] for row in control['rows']] == [
        'Node 101', 'Node 104', 'Node 110', 'Node 113',
        'Node 1301', 'Node 1304', 'Node 1310', 'Node 1313']
    [cell] = control['rows'][0]['cells']
    [figure] = cell
    assert figure['kind'] == 'plot' and figure['caption'] == '101Z+'
    assert [ch['label'] for ch in figure['channels']] == ['101Z+']
    assert 'note' not in figure, 'on its axis: nothing to note'
    assert control['label'].startswith('Figure '), 'a grid is one numbered figure'
    text = ' '.join(b['html'] for b in payload['blocks'] if b['kind'] == 'text')
    assert 'a row per node, a column per direction' in text
    assert not re.search(r'\{\{figure:', text), 'every reference resolves'
    # without a geometry the column is the DOF's own letter
    bare = hertzforge.random_vibration_run(fixture_path('plate', 'random.nc4'))
    payload = payload_of(bare, random_template(bare, object_groups=bare.object_groups))
    control = [b for b in payload['blocks'] if b['kind'] == 'grid'][1]
    assert control['columns'] == ['Z']


def test_the_grid_lays_channels_by_node_and_direction():
    """The layout on its own: rows in the order the nodes first appear,
    global columns for channels a geometry can place — with the angle
    off the axis noted — the DOF's letter for ones it cannot, and a
    'Channel' column for a node number alone."""
    from hertzforge.core.report import channel_grid

    c, s = np.cos(np.radians(30.0)), np.sin(np.radians(30.0))
    turned = [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]]
    identity = [[1.0, 0, 0], [0, 1.0, 0], [0, 0, 1.0], [0, 0, 0]]
    geometry = hertzforge.Geometry(
        node_id=[101, 102], node_xyz=np.zeros((2, 3)), node_disp_cs=[2, 1],
        cs_id=[1, 2], cs_name=['', ''], cs_type=[0, 0],
        cs_matrix=[identity, turned])
    labels = ['101X+', '101Y+', '102Z-', '101Z+', '300X+', '400']
    grid = channel_grid(labels, geometry)
    assert grid['columns'] == ['Global X', 'Global Y', 'Global Z', 'X', 'Channel']
    assert [row['label'] for row in grid['rows']] == [
        'Node 101', 'Node 102', 'Node 300', 'Node 400']
    node101 = grid['rows'][0]['cells']
    assert node101[0] == [{'channel': '101X+', 'note': '30° off global X'}]
    assert node101[1] == [{'channel': '101Y+', 'note': '30° off global Y'}]
    assert node101[2] == [{'channel': '101Z+', 'note': ''}]
    assert node101[3] == node101[4] == []
    assert grid['rows'][1]['cells'][2] == [{'channel': '102Z-', 'note': ''}]
    assert grid['rows'][2]['cells'][3] == [{'channel': '300X+', 'note': ''}], \
        'a node the geometry lacks stands under its own letter'
    assert grid['rows'][3]['cells'][4] == [{'channel': '400', 'note': ''}], \
        'a node number alone is a node, in the channel column'
    # no geometry at all: letters and the channel column only
    grid = channel_grid(['5Z+', '5X+', '6'])
    assert grid['columns'] == ['X', 'Z', 'Channel']
    assert grid['rows'][0]['cells'] == [[{'channel': '5X+', 'note': ''}],
                                        [{'channel': '5Z+', 'note': ''}], []]


def test_the_page_draws_a_grid_of_cells():
    """The page lays a grid figure out as one section — cells are
    divs, because edit mode maps sections to blocks by position — and
    draws each cell with the plot's own code."""
    from hertzforge.report.page import _CSS, _JS

    assert 'function gridBlock(block) {' in _JS
    assert "else if (block.kind === 'grid') gridBlock(block);" in _JS
    assert 'function plotBlock(block, into) {' in _JS
    assert 'cell.forEach(figure => plotBlock(figure, holder));' in _JS
    assert "holder.className = 'gridcell'" in _JS
    assert '.grid { display: grid;' in _CSS


def test_sections_whose_objects_are_absent_are_dropped():
    """A run with no specification has nothing to compare against
    (Brandon, 2026-09-19): the specification, control, compliance and
    octave sections go, heading and prose too, rather than standing
    as headings over nothing. An empty project keeps the whole
    outline — that is the template being read."""
    from hertzforge.core.report import random_template

    thin = hertzforge.Project()
    thin.import_file(fixture_path('plate', 'random.nc4'))
    thin.remove('Specification')
    thin.compute_psds('Time History')

    def headings(report):
        return [b['text'].split('\n')[0][3:] for b in report.blocks
                if b.get('kind') == 'text' and b['text'].startswith('## ')]

    assert headings(random_template(thin, object_groups=thin.object_groups)) == [
        'Test Summary', 'Test Article and Instrumentation', 'Measured Data',
        'Data Quality', 'Conclusions']
    full = hertzforge.random_vibration_run(fixture_path('plate', 'random.nc4'))
    # 'Specification' as its own heading went with the figure it
    # introduced; the requirement is read on the Control comparison
    assert 'Control' in headings(random_template(full, object_groups=full.object_groups))
    assert 'Specification' not in headings(random_template(full, object_groups=full.object_groups))
    assert 'Octave Band Comparison' in headings(random_template({}))
    outline = random_template({}).blocks
    assert {'scene', 'photo'} <= {b['kind'] for b in outline}, \
        'an empty project keeps every slot of the outline'
    assert len(outline) > len(random_template(full, object_groups=full.object_groups).blocks), \
        'a run without a geometry or photographs keeps only what binds'


def test_the_figures_open_on_the_band_the_target_is_written_on():
    """A controller's target sits on every FFT line to Nyquist, NaN
    outside the band it controlled — the plate's on 726 lines of
    1025 — so opening on the specification's *axis* opened on the
    whole run (Brandon, 2026-09-19: "zoomed across the entire data
    range"). Both the specification figure and the comparison open on
    the lines the target is written on; the rest is a zoom away."""
    import json
    import re

    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    psds = loaded['time_data'].compute_psds()
    y = np.real(np.asarray(spec.ordinate)[0])
    written = np.flatnonzero(np.isfinite(y) & (y > 0))
    band = [float(spec.abscissa[written[0]]), float(spec.abscissa[written[-1]])]
    assert band == [50.0, 1500.0] and float(spec.abscissa[-1]) == 2048.0, \
        'the fixture is the case: written on a band, stored to Nyquist'
    objects = {'S': spec, 'P': psds}
    alone = _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves',
                         'channel': '101Z+'}, spec, objects, hertzforge.SI)
    assert alone['home_x'] == band
    assert max(alone['x']) == 2048.0, 'the stored axis is still there to zoom to'
    against = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                           'mode': 'curves', 'channel': '101Z+'},
                          psds, objects, hertzforge.SI)
    assert against['home_x'] == band
    # and in the grid's cells, which are those very figures
    project = hertzforge.random_vibration_run(fixture_path('plate', 'random.nc4'))
    page = render_html(random_template(project, object_groups=project.object_groups), project,
                       object_groups=project.object_groups, unit_system=hertzforge.SI)
    payload = json.loads(re.search(
        r'<script id="data"[^>]*>(.*?)</script>', page, re.DOTALL).group(1))
    grids = {b['caption']: b for b in payload['blocks'] if b['kind'] == 'grid'}
    for row in grids['Control against specification']['rows']:
        [[cell]] = row['cells']
        assert cell['home_x'] == band, row['label']
    # the banded comparison opens on the same band, read in decades
    # because its axis is logarithmic
    for row in grids['Control against specification, octave bands']['rows']:
        [[cell]] = row['cells']
        assert 10 ** cell['home_x'][0] < band[0]
        assert 10 ** cell['home_x'][1] > band[1]


def test_the_front_matter_says_only_what_the_project_holds():
    """A run with no geometry kept a paragraph about "the test
    geometry" and four unbound scene cards (Brandon, 2026-09-19): the
    scenes go, and the prose is the channel table's sentence — the
    photographs' too when there are any. With a geometry the standing
    text stands; an empty project keeps it as the outline."""
    from hertzforge.core.report import _INSTRUMENTATION_TEXT, instrumentation_text

    bare = hertzforge.random_vibration_run(fixture_path('plate', 'random.nc4'))
    template = random_template(bare, object_groups=bare.object_groups)
    assert not [b for b in template.blocks if b['kind'] in ('scene', 'photo')]
    prose = instrumentation_text(bare, bare.object_groups)
    assert prose.startswith('## Test Article and Instrumentation\n\n')
    assert 'geometry' not in prose and 'photograph' not in prose
    assert '{{table:Instrumentation}}' in prose
    assert [b['text'] for b in template.blocks
            if b['kind'] == 'text'][1] == prose
    page = render_html(template, bare, object_groups=bare.object_groups, edit=True,
                       unit_system=hertzforge.SI)
    assert "'kind': 'unbound'" not in page and '"kind": "unbound"' not in page, \
        'no unbound cards for the editor to show'
    from hertzforge.core.photos import Photos

    with_photos = hertzforge.random_vibration_run(fixture_path('plate', 'random.nc4'))
    photos = Photos()
    photos.names, photos.formats, photos.images = ['front'], ['png'], [b'']
    with_photos.add('Photos', photos)
    with_photos.link('Time History', 'Photos')
    assert 'photographs record the article' in instrumentation_text(
        with_photos, with_photos.object_groups)
    placed = hertzforge.random_vibration_run(
        fixture_path('plate', 'random.nc4'),
        geometry=fixture_path('plate', 'geometry.npz'), length_unit='m')
    assert instrumentation_text(placed, placed.object_groups) == _INSTRUMENTATION_TEXT
    assert instrumentation_text({}) == _INSTRUMENTATION_TEXT


def test_no_reference_is_left_as_written_on_a_thin_projects_page():
    """A thin project's report showed `{{figure:...}}` in its
    conclusions, and `{{@basis:Psd....}}` in its summary, for what it
    did not have (Brandon, 2026-09-19): the conclusions reference no
    figures, and the summary and data-quality prose speak only of what
    binds — so no reference of any kind reaches the page as written.
    The full run keeps every sentence."""
    import re

    from hertzforge.core.report import random_template

    for objects in ({}, hertzforge.random_vibration_run(
            fixture_path('plate', 'random.nc4'))):
        object_groups = getattr(objects, 'links', None)
        [conclusions] = [b['text'] for b in random_template(objects, object_groups).blocks
                         if b.get('kind') == 'text'
                         and b['text'].startswith('## Conclusions')]
        assert '{{figure:' not in conclusions and '{{table:' not in conclusions

    def page_of(project):
        return render_html(random_template(project, object_groups=project.object_groups),
                           project, object_groups=project.object_groups,
                           unit_system=hertzforge.SI)

    run_only = hertzforge.Project()
    run_only.import_file(fixture_path('plate', 'random.nc4'))
    thin = hertzforge.Project()
    thin.import_file(fixture_path('plate', 'random.nc4'))
    thin.remove('Specification')
    thin.compute_psds('Time History')
    for project in (run_only, thin):
        page = page_of(project)
        assert 'Conclusions' in page
        assert not re.search(r'\{\{[^}]*\}\}', page), \
            'nothing unresolved reaches the page'
    assert 'One check stands behind' in page_of(thin), 'kurtosis alone'
    assert 'overlap.' in page_of(run_only) and 'resolution out to' not in page_of(run_only)
    full = page_of(hertzforge.random_vibration_run(fixture_path('plate', 'random.nc4')))
    assert 'Two checks stand behind' in full and 'Multiple coherence (Figure' in full
    assert 'resolution out to 2048 Hz' in full


def test_the_octave_marks_are_the_lines_the_table_counts():
    """The page judged a banded comparison with `outside`'s defaults
    where the table read it bin against bin (Brandon, 2026-09-19: the
    last octave band marked blue under a response in the middle of
    its zone). One reading — `compliance.how_read` — for both: a
    response on its target is unmarked, and the real pair's marks
    are the table's counts, channel by channel."""
    from conftest import banded_pair_on_target

    from hertzforge.core.compliance import compare_all
    from hertzforge.report import _plot_block

    banded, on_target = banded_pair_on_target()
    built = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                         'mode': 'curves'}, on_target,
                        {'S': banded, 'P': on_target}, hertzforge.SI)
    for channel in built['channels']:
        assert all(v is None for key in ('over', 'under')
                   for v in channel.get(key, [])), channel['label']
    project = hertzforge.random_vibration_run(fixture_path('plate', 'random.nc4'))
    _narrow, octave = project.psds
    _spec, octave_spec = project.specifications
    counted = dict(compare_all(octave_spec, octave))
    built = _plot_block({'kind': 'plot', 'source': 'P', 'specification': 'S',
                         'mode': 'curves'}, octave,
                        {'S': octave_spec, 'P': octave}, hertzforge.SI)
    for channel in built['channels']:
        marked = sum(1 for key in ('over', 'under')
                     for v in channel.get(key, []) if v is not None)
        assert marked == counted[channel['label']]['abort_lines'], channel['label']


def test_a_table_row_is_one_line():
    """A channel table's comments wrapped every row onto four lines
    (Brandon, 2026-09-18): cells never wrap, and a table wider than the
    page scrolls in its own wrap."""
    from hertzforge.report.page import _CSS, _JS

    rule = _CSS[_CSS.index('th, td {'):]
    rule = rule[:rule.index('}')]
    assert 'white-space: nowrap' in rule
    assert '.tablewrap { overflow-x: auto; }' in _CSS
    assert "wrap.className = 'tablewrap'" in _JS


def test_a_specification_figure_may_name_its_channel():
    """A bounded source's block that names a channel carries that one
    channel and no drop-down note."""
    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = loaded['Random_specification']
    named = _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves',
                         'channel': '104Z+', 'caption': 'Test specification — 104Z+'},
                        spec, {'S': spec}, hertzforge.SI)
    assert [ch['label'] for ch in named['channels']] == ['104Z+']
    assert named['caption'] == 'Test specification — 104Z+'
    assert _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves',
                        'channel': '999Z+'}, spec, {'S': spec},
                       hertzforge.SI) is None


def test_the_page_steps_the_zones_with_the_target():
    """The report drew a banded specification\'s limits as polygons
    through the band centers under a stepped target (Brandon,
    2026-09-19): the page steps a zone on the same edges as its target."""
    from hertzforge.report.page import _JS

    assert 'const stepped = !!(zoneSteps && zoneEdges);' in _JS
    assert 'if (stepped) g.lineTo(xAt(i, 1), YL);' in _JS


def test_a_banded_figure_keeps_its_zones_on_the_block_grid():
    """The zones step with the target, and a curve that claims its own
    grid without edges takes that away.

    `_decimate` rounds every coordinate to `PAGE_DIGITS`, and an octave
    band center is a geometric mean, so the rounded copy of the block's
    own grid compared unequal to it and every octave figure shipped a
    curve carrying an `x` and no `edges`. The page reads that pair as
    "on its own grid, edges unknown" and drew the warning and abort
    zones as a smooth polygon while the target beside them stepped
    (Brandon, 2026-09-21). The comparison figure was right all along
    because it sets both.
    """
    from hertzforge.report import _plot_block

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    banded = loaded['Random_specification'].to_octave(6)
    block = _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves',
                         'channel': '101Z+'}, banded, {'S': banded},
                        hertzforge.SI)
    assert block['steps'] and block['edges'], 'a banded target steps'
    assert block['curves'][0].get('x') is None, (
        'on the block grid, so the page reads the block edges'
    )
    assert len(block['edges']) == len(block['x']) + 1
    zones = block['channels'][0]['zones']
    assert [z['severity'] for z in zones] == ['warning', 'abort',
                                              'warning', 'abort']
    for zone in zones:
        for edge in (zone['lower'], zone['upper']):
            if edge is not None:
                assert len(edge) == len(block['x']), (
                    'every zone edge sits on the grid the target steps on'
                )


def _without_force(project):
    """The placed plate run with its force channels removed: a test
    controlled on accelerometers alone."""
    project['Time History'].delete_records(dim='force')
    return project


def test_a_run_with_no_force_channels_cites_no_force_figure():
    """A random report printed '({{figure:Excitation degrees of freedom
    (force)}})' for a run with no force channels (Brandon, 2026-10-27):
    the paragraph was fixed text and the figure is built only for a
    quantity the run measured. It now names the figures there are."""
    import re

    project = _without_force(hertzforge.random_vibration_run(
        fixture_path('plate', 'random.nc4'),
        geometry=fixture_path('plate', 'geometry.npz'), length_unit='m'))
    template = random_template(project, object_groups=project.object_groups)
    captions = [b['caption'] for b in template.blocks if b['kind'] == 'scene']
    assert captions == ['Test geometry and measurement locations',
                        'Response degrees of freedom (acceleration)']
    [prose] = [b['text'] for b in template.blocks if b['kind'] == 'text'
               and b['text'].startswith('## Test Article')]
    assert 'excitation' not in prose.lower()
    assert ('The response degrees of freedom ({{figure:Response degrees of '
            'freedom (acceleration)}}) mark every measured accelerometer '
            'direction.') in prose
    page = render_html(template, project, object_groups=project.object_groups,
                       unit_system=hertzforge.SI)
    assert not re.search(r'\{\{[^}]*\}\}', page), \
        'nothing unresolved reaches the page'


@pytest.mark.parametrize('kind', ['modal', 'random', 'transient', 'sine',
                                  'shock', 'sysid'])
def test_every_template_cites_only_dof_figures_it_built(kind):
    """Every template that refers to the DOF figures refers to the ones
    `front_matter` built, on a run with no force channels."""
    import re

    from hertzforge.core.report import TEMPLATE_BUILDERS

    project = _without_force(hertzforge.random_vibration_run(
        fixture_path('plate', 'random.nc4'),
        geometry=fixture_path('plate', 'geometry.npz'), length_unit='m'))
    report = TEMPLATE_BUILDERS[kind](project, project.object_groups)
    captions = [b['caption'] for b in report.blocks if b['kind'] == 'scene']
    text = ' '.join(b['text'] for b in report.blocks if b['kind'] == 'text')
    cited = re.findall(r'\{\{figure:([^}]*degrees of freedom[^}]*)\}\}', text)
    assert cited, 'the template cites a DOF figure'
    for reference in cited:
        assert any(c.lower().startswith(reference.strip().lower())
                   for c in captions), f'{reference!r} has no figure'


def test_the_dof_sentence_names_what_there_is():
    from hertzforge.core.report import dof_sentence

    both = dof_sentence('{{figure:E}}', '{{figure:R}}', 'how it was driven',
                        'what was read')
    assert both == (' The excitation degrees of freedom ({{figure:E}}) mark '
                    'where and how it was driven; the response degrees of '
                    'freedom ({{figure:R}}) mark what was read.')
    assert dof_sentence('{{figure:E}}', '', 'a', 'b') == (
        ' The excitation degrees of freedom ({{figure:E}}) mark where and a.')
    assert dof_sentence('', '', 'a', 'b') == ''
