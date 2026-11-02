"""Reports: blocks, templates, rendering, and the builder.

The renderer's output is executed in a real browser engine (QtWebEngine
is Chromium) — a report whose JavaScript fails to build its viewers is
a broken deliverable however valid its Python was.
"""

from __future__ import annotations

import json

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge import io
from hertzforge.core.report import Report, modal_template
from hertzforge.report import render_html


def test_blocks_move_the_way_a_drag_drops():
    report = Report('R', [{'kind': 'text', 'text': label}
                          for label in 'abcde'])
    report.move([1, 3], 0)
    assert [b['text'] for b in report.blocks] == list('bdace')
    report.move([0], 5)
    assert [b['text'] for b in report.blocks] == list('daceb')
    # the shift case: lifting rows from above the target must land the
    # block where the *user dropped it*, not one-per-lifted-row later
    fresh = Report('R', [{'kind': 'text', 'text': label}
                         for label in 'abcde'])
    fresh.move([0, 1], 3)
    assert [b['text'] for b in fresh.blocks] == list('cabde')


def test_a_report_round_trips_through_vibe(tmp_path, project):
    report = modal_template(project)
    report.blocks[0]['text'] = 'Custom prose survives.'
    path = tmp_path / 'report.vdyn'
    io.save(report, str(path))
    back = io.load(str(path))
    assert isinstance(back, Report)
    assert back.title == report.title
    assert back.blocks == report.blocks, 'order and text intact — a template'


def test_unbound_blocks_are_slots_not_errors(project):
    report = modal_template(project)
    index = next(i for i, block in enumerate(report.blocks)
                 if block.get('mode') == 'cmif')
    report.blocks[index]['source'] = 'Someone Elses FRF'
    assert index in report.unbound(project)
    html = render_html(report, project)
    assert 'Someone Elses FRF' not in html, 'an unbound block does not render'


def test_symbolic_bindings_resolve_through_the_links(project):
    """Templates bind '@basis:Frf' and friends: whatever the objects
    are called, the Basis group's own lead every binding at render
    time — and without any links, first-of-type stands in."""
    from hertzforge.core.report import resolve_binding

    fem_first = {'Model Modes': project['Shape Set'],
                 'Model FRF': project['FRF'],
                 'Run 7 Modes': project['Shape Set'],
                 'Run 7 FRF': project['FRF']}
    object_groups = [{'members': ['Run 7 Modes', 'Run 7 FRF'],
              'role': 'Basis'}]
    report = modal_template(fem_first, object_groups=object_groups)
    cmif = next(b for b in report.blocks if b.get('mode') == 'cmif')
    assert cmif['source'] == '@basis:Frf', 'no name in the binding'
    assert resolve_binding(cmif['source'], fem_first, object_groups) == \
        'Run 7 FRF'
    assert resolve_binding(cmif['shapes'], fem_first, object_groups) == \
        'Run 7 Modes', (
        'the resynthesis comes from the basis modes, not the model')
    assert resolve_binding('@other:ShapeSet', fem_first, object_groups) == \
        'Model Modes', 'the other side: outside the Basis group'
    # the rendered payload proves render-time resolution end to end
    html = render_html(report, fem_first, object_groups=object_groups)
    assert 'Model FRF' not in html.split('type="application/json">')[0]
    payload = json.loads(html.split(
        'type="application/json">')[1].split('</script>')[0])
    assert any(b.get('kind') == 'plot' for b in payload['blocks'])
    # without links, symbolic degrades: first of the type binds
    assert resolve_binding('@basis:Frf', fem_first) == 'Model FRF'
    assert resolve_binding('@other:ShapeSet', fem_first) == \
        'Run 7 Modes', 'no Basis declared: the second of the type'
    assert resolve_binding('@basis:Frf', {}) is None
    assert resolve_binding('Some Name', fem_first) == 'Some Name', (
        'literal names pass through untouched')


def test_drive_plots_mark_the_named_sets_modes():
    """A curves plot that names a shape set carries its frequencies
    as marks — the fitting screen's bookmarks over the drive FRFs."""
    from hertzforge.core.data import Frf
    from hertzforge.core.shapes import ShapeSet

    frf = Frf(abscissa=np.linspace(0, 128, 64),
              ordinate=np.ones((2, 64), dtype=complex),
              response_dof=['1Z+', '2Z+'], reference_dof=['1Z+'] * 2,
              ordinate_dim='acceleration/force',
              ordinate_unit='m/s**2', reference_unit='N')
    shapes = ShapeSet([12.5, 47.0], [0.01, 0.02], ['1Z+', '2Z+'],
                      np.ones((2, 2)))
    objects = {'FRF': frf, 'Shapes': shapes}
    report = Report('r', [{'kind': 'plot', 'source': 'FRF',
                           'mode': 'curves', 'select': 'drive',
                           'component': 'imag', 'shapes': 'Shapes',
                           'caption': ''}])
    payload = json.loads(render_html(report, objects).split(
        'type="application/json">')[1].split('</script>')[0])
    plot = payload['blocks'][0]
    assert plot['marks'] == [12.5, 47.0]
    assert len(plot['curves']) == 1, 'the drive diagonal only'


def test_the_file_is_self_contained(project):
    html = render_html(modal_template(project), project)
    assert 'http://' not in html and 'https://' not in html, (
        'no network, no CDN — it must open on a locked-down machine')
    assert 'cmif' in html.lower()


def test_text_placeholders_fill_from_the_live_objects(project):
    """{{Object.field}} in report text resolves at render time — the
    template's summary fills itself in whatever project it lands in —
    and anything unresolvable stays visible as written."""
    from hertzforge.core.data import TimeHistory

    time = TimeHistory(np.arange(256) / 256.0, np.ones((8, 256)),
                       response_dof=['1X+', '2X+'] * 4,
                       block=[f'avg {i}' for i in (1, 1, 2, 2, 3, 3, 4, 4)])
    objects = dict(project, Time=time)
    report = Report('R', [{'kind': 'text', 'text':
        'Sampled at {{Time.sample_rate}}: {{Time.num_channels}} channels '
        'x {{Time.num_averages}} averages of {{Time.num_samples}} '
        'samples ({{Time.frequency_resolution}} resolution). '
        '{{Shape Set.num_modes}} modes up to {{Shape Set.max_frequency}}. '
        'Unresolved: {{Time.no_such_field}} and {{Nobody.num_samples}}.'}])
    html = render_html(report, objects)
    text = _payload(html)['blocks'][0]['html']
    assert 'Sampled at 256 Hz: 2 channels x 4 averages of 256 samples '\
           '(1 Hz resolution).' in text
    shapes = project['Shape Set']
    assert f'{shapes.num_shapes} modes up to '\
           f'{float(np.max(shapes.frequency)):g} Hz.' in text
    # a reference the page cannot fill never reaches a reader: the
    # sentence holding it is left out, and the edit page marks it
    # (2026-10-28; it was printed as written)
    assert 'Unresolved' not in text and '{{' not in text
    edited = _payload(render_html(report, objects, edit=True))['blocks'][0]
    assert '[no value for Time.no_such_field]' in edited['html']
    assert '[no value for Nobody.num_samples]' in edited['html']


def test_figure_references_follow_the_numbering(project):
    """{{figure:<start of caption>}} in text reads the number the
    figure carries right now — insert a figure above it and the
    reference renumbers itself on the next render. A sentence citing a
    figure that is not there is left out of the reading page and marked
    on the edit page (2026-10-27; it was left as written before, and
    reached readers as '{{figure:...}}')."""
    report = Report('R', [
        {'kind': 'text', 'text': 'See {{figure:Auto-MAC}} and '
         '{{table:Modes}}. {{figure:Nothing here}} is gone.'},
        {'kind': 'plot', 'source': 'Shape Set', 'mode': 'mac',
         'caption': 'Auto-MAC of the shapes'},
        {'kind': 'table', 'source': 'Shape Set', 'caption': 'Modes'},
    ])
    text = _payload(render_html(report, project))['blocks'][0]['html']
    assert 'See Figure 1 and Table 1.' in text
    assert 'Nothing here' not in text and 'is gone' not in text, (
        'the sentence citing a missing figure is left out')
    edited = _payload(render_html(report, project, edit=True))
    [html] = [b['html'] for b in edited['blocks'] if b['kind'] == 'text']
    assert 'no figure starts' in html and 'is gone' in html, (
        'the editor marks it instead')
    report.add({'kind': 'scene', 'geometry': 'Geometry', 'shapes': '',
                'caption': 'The geometry'}, at=1)
    text = _payload(render_html(report, project))['blocks'][0]['html']
    assert 'See Figure 2 and Table 1.' in text, (
        'the reference renumbered with the new figure above it')


def test_markdown_renders_and_cannot_smuggle_markup():
    from hertzforge.report.markdown import to_html

    html = to_html('## Setup\n\nThe **fixture** was *free-free*:\n'
                   '- bungees\n- 4 shakers\n\n<script>alert(1)</script>')
    assert '<h3>Setup</h3>' in html
    assert '<strong>fixture</strong>' in html
    assert '<em>free-free</em>' in html
    assert '<ul>' in html and html.count('<li>') == 2
    assert '<script>' not in html, 'typed markup is text, not markup'
    assert '&lt;script&gt;' in html


def test_edit_mode_carries_the_selection_and_the_export_never_does(project):
    """The editor's page keeps one piece of chrome — the frame on the
    selected block and the click that selects it (2026-09-08: every act
    is on the bar and the pane) — and the export carries none."""
    report = modal_template(project)
    edited = render_html(report, project, edit=True, selected=2)
    exported = render_html(report, project)
    assert 'qwebchannel' in edited and '__select' in edited
    assert '"selected": 2' in edited
    assert 'insertbar' not in edited and 'blockbar' not in edited, \
        'no chrome in the page: the acts are on the bar'
    assert 'qwebchannel' not in exported and 'qrc:' not in exported
    assert '__select' not in exported and 'selected' not in _payload(exported)


def test_edit_mode_keeps_unbound_blocks_as_cards(project):
    report = modal_template(project)
    report.blocks[2]['source'] = 'Nobody'
    edited = render_html(report, project, edit=True)
    payload = json.loads(
        edited.split('type="application/json">')[1].split('</script>')[0])
    kinds = [b['kind'] for b in payload['blocks']]
    assert 'unbound' in kinds, 'a slot to rebind, not a silent hole'
    assert all('index' in b for b in payload['blocks']), \
        'a click on any figure of a block selects the block'


def test_scenes_carry_element_faces(project):
    """Elements draw as shaded faces in the report, not just edge lines —
    the plate's 144 quads all arrive with four corners. A perimeter
    line of beams goes on first, because the meshed plate ships without
    any and the scene must carry both kinds at once."""
    project['Geometry'].add_beams([101, 113, 1313, 1301, 101])
    report = Report('R', [{'kind': 'scene', 'geometry': 'Geometry',
                           'shapes': 'Shape Set', 'caption': ''}])
    html = render_html(report, project)
    payload = json.loads(
        html.split('type="application/json">')[1].split('</script>')[0])
    scene = payload['blocks'][0]
    assert len(scene['faces']) == 144
    assert all(len(face['nodes']) == 4 for face in scene['faces'])
    assert scene['lines'], 'drawn lines still draw as lines'


def test_figures_and_tables_number_sequentially(project):
    """Figure 1, Figure 2, ... over what actually renders; tables count
    on their own; an unbound block never claims a number."""
    report = modal_template(project)
    index = next(i for i, block in enumerate(report.blocks)
                 if block.get('mode') == 'cmif')
    report.blocks[index]['source'] = 'Nobody'      # unbind the CMIF
    html = render_html(report, project)
    payload = json.loads(
        html.split('type="application/json">')[1].split('</script>')[0])
    labels = [b.get('label') for b in payload['blocks']]
    figures = [label for label in labels
               if label and label.startswith('Figure')]
    tables = [label for label in labels if label and label.startswith('Table')]
    assert figures == [f'Figure {i + 1}' for i in range(len(figures))], (
        'the skipped CMIF left no gap in the numbering')
    assert tables == ['Table 1']
    # no units declared on the fixture, so the DOFs scenes stay slots
    assert len(figures) == 5, ('geometry scene, drive magnitude, drive '
                               'imaginary, MAC, and the shapes scene')


def test_scenes_carry_the_guis_palette(project):
    """The report is the GUI, emailed: nodes, tracelines and elements
    arrive with the same palette colors the desktop scene paints."""
    report = Report('R', [{'kind': 'scene', 'geometry': 'Geometry',
                           'shapes': 'Shape Set', 'caption': ''}])
    html = render_html(report, project)
    payload = json.loads(
        html.split('type="application/json">')[1].split('</script>')[0])
    scene = payload['blocks'][0]
    assert len(scene['node_colors']) == len(scene['points'])
    assert all(c.startswith('#') for c in scene['node_colors'])
    assert all(len(line) == 3 and line[2].startswith('#')
               for line in scene['lines']), 'every line knows its color'
    assert all(face['color'].startswith('#') for face in scene['faces'])
    assert all(mode['peak'] > 0 for mode in scene['modes']), (
        'the colormap scale is pinned to each mode, as in the GUI')


def _payload(html):
    return json.loads(
        html.split('type="application/json">')[1].split('</script>')[0])


def test_result_blocks_filter_records_the_guis_way(project):
    """'select' mirrors the GUI's own filters: drive is the FRF
    diagonal, dim:<quantity> splits a time history by what it holds."""
    from hertzforge.core.data import TimeHistory

    time = TimeHistory(np.linspace(0, 1, 50), np.ones((3, 50)),
                       response_dof=['1X+', '2X+', '3X+'])
    time.define_units({0: 'N', 1: 'g', 2: 'g'})
    objects = dict(project, Time=time)
    frf = project['FRF']
    diagonal = [i for i in range(frf.num_records)
                if frf.response_dof[i] == frf.reference_dof[i]]
    assert 0 < len(diagonal) < frf.num_records, 'a real subset to falsify'
    report = Report('R', [
        {'kind': 'plot', 'source': 'FRF', 'mode': 'curves',
         'select': 'drive', 'caption': ''},
        {'kind': 'plot', 'source': 'Time', 'mode': 'curves',
         'select': 'dim:force', 'caption': ''},
        {'kind': 'plot', 'source': 'Time', 'mode': 'curves',
         'select': 'dim:acceleration', 'caption': ''},
        {'kind': 'plot', 'source': 'Time', 'mode': 'curves',
         'select': 'dim:voltage', 'caption': ''},
    ])
    blocks = _payload(render_html(report, objects))['blocks']
    assert len(blocks) == 3, 'no voltage records, so no voltage block'
    assert len(blocks[0]['curves']) == len(diagonal)
    assert [c['label'] for c in blocks[1]['curves']] == ['1X+']
    assert [c['label'] for c in blocks[2]['curves']] == ['2X+', '3X+']


def test_the_insert_menu_offers_the_time_quantities_that_exist(project):
    """Time (Force), Time (Acceleration), … — one Result option per
    quantity the project's time data actually holds, none otherwise."""
    from hertzforge.core.data import TimeHistory
    from hertzforge.core.report import insert_options

    labels = [label for _k, label in insert_options(project)]
    assert not any(label.startswith('Time') for label in labels)
    time = TimeHistory(np.linspace(0, 1, 50), np.ones((2, 50)),
                       response_dof=['1X+', '2X+'])
    time.define_units({0: 'N', 1: 'g'})
    options = insert_options(dict(project, Time=time))
    assert ('time:acceleration', 'Time (Acceleration)') in options
    assert ('time:force', 'Time (Force)') in options
    # the editor's Insert menu offers them (test_report_bar); the page
    # no longer carries an insert bar of its own
    payload = _payload(render_html(
        Report('R'), dict(project, Time=time), edit=True))
    assert 'inserts' not in payload


def test_frf_component_blocks_read_the_complex_data_four_ways(project):
    from hertzforge.core.report import insert_options
    from hertzforge.units import DEFAULT_SYSTEM

    labels = [label for _k, label in insert_options(project)]
    assert [l for l in labels if l.startswith('FRF - Drive Point')] == [
        'FRF - Drive Point - Magnitude', 'FRF - Drive Point - Real',
        'FRF - Drive Point - Imaginary', 'FRF - Drive Point - Phase']
    frf = project['FRF']
    diagonal = [i for i in range(frf.num_records)
                if frf.response_dof[i] == frf.reference_dof[i]]
    report = Report('R', [
        {'kind': 'plot', 'source': 'FRF', 'mode': 'curves',
         'select': 'drive', 'component': component, 'caption': ''}
        for component in ('imag', 'real', 'phase', 'magnitude')])
    blocks = _payload(render_html(report, project))['blocks']
    values = np.asarray(frf.display_ordinate(DEFAULT_SYSTEM, diagonal))
    assert blocks[0]['logy'] is False
    assert blocks[0]['ylabel'].endswith('(imaginary)')
    assert np.allclose(blocks[0]['curves'][0]['y'], values[0].imag)
    assert np.allclose(blocks[1]['curves'][0]['y'], values[0].real)
    assert blocks[2]['ylabel'] == 'phase [deg]'
    assert np.allclose(blocks[2]['curves'][0]['y'],
                       np.degrees(np.angle(values[0])))
    assert blocks[3]['logy'] is True, 'magnitude keeps the log axis'


def test_a_channel_table_block_shows_the_whole_schema(project):
    """The report used to cut acquisition plumbing — device, coupling,
    input ranges — out of a table that carried anything. The schema is
    that cut now: a Rattlesnake save's wiring stops at the importer, so
    the report shows every column and there is no second list to drift
    from the first."""
    import hertzforge
    from hertzforge.core.channel_table import ChannelTable, title_of

    contents = hertzforge.import_file(fixture_path('plate', 'modal.nc4'))
    table = next(obj for obj in contents.values()
                 if isinstance(obj, ChannelTable))
    assert 'physical_device' not in table.column_names, (
        'the controller wiring stops at the importer')
    objects = dict(project, Channels=table)
    report = Report('R', [{'kind': 'table', 'source': 'Channels',
                           'caption': 'Instrumentation'}])
    block = _payload(render_html(report, objects))['blocks'][0]
    # and the direction columns a basis geometry derives beside them
    # (2026-09-20), when the report has one to read against
    from hertzforge.core.channel_table import DERIVED_COLUMNS
    from hertzforge.core.report import resolve_binding

    placed = resolve_binding('@basis:Geometry', objects, None) in objects
    derived = list(DERIVED_COLUMNS) if placed else []
    assert block['headers'] == [title_of(n) for n in (*table.SCHEMA, *derived)]
    assert len(block['rows']) == table.num_channels


def test_a_coherence_block_renders_as_the_pinned_map(project):
    from hertzforge.core.data import MultipleCoherence

    values = np.linspace(-0.1, 1.1, 40).reshape(2, 20)
    coherence = MultipleCoherence(np.linspace(0, 190, 20), values,
                                  response_dof=['1X+', '2X+'])
    report = Report('R', [{'kind': 'plot', 'source': 'Coh',
                           'mode': 'map', 'caption': 'coherence'}])
    block = _payload(render_html(report, {'Coh': coherence}))['blocks'][0]
    assert block['kind'] == 'map'
    assert block['label'] == 'Figure 1', 'a map numbers as a figure'
    assert block['labels'] == ['1X+', '2X+']
    flat = [v for row in block['rows'] for v in row]
    assert min(flat) == 0.0 and max(flat) == 1.0, 'pinned to 0..1'
    assert len(block['x']) == 20 and len(block['rows'][0]) == 20


def test_a_cmif_block_overlays_the_modal_synthesis(project):
    """CMIF measured against CMIF synthesized, dashed and pair-colored —
    the fitting screen's judgment, in the report."""
    report = Report('R', [{'kind': 'plot', 'source': 'FRF',
                           'mode': 'cmif', 'shapes': 'Shape Set',
                           'caption': ''}])
    html = render_html(report, project)
    payload = json.loads(
        html.split('type="application/json">')[1].split('</script>')[0])
    curves = payload['blocks'][0]['curves']
    measured = [c for c in curves if not c.get('dash')]
    dashed = [c for c in curves if c.get('dash')]
    assert len(measured) == 4 and len(dashed) == 4
    assert [c['color'] for c in dashed] == [c['color'] for c in measured]
    # the survey FRFs are exactly modal: synthesis matches measurement
    top = np.array([v for v in measured[0]['y'] if v is not None])
    twin = np.array([v for v in dashed[0]['y'] if v is not None])
    assert np.allclose(top, twin, atol=1e-6)
    # and the plot opens on the mode band: 80% of the lowest fitted
    # frequency to 120% of the highest
    freqs = project['Shape Set'].frequency
    assert payload['blocks'][0]['home_x'] == pytest.approx(
        [0.8 * float(np.min(freqs)), 1.2 * float(np.max(freqs))])
    # and carries the fitting screen's gray bookmarks, coincident
    # frequencies collapsed to one
    assert payload['blocks'][0]['marks'] == pytest.approx(
        list(dict.fromkeys(float(f) for f in freqs)))
    bare = Report('R', [{'kind': 'plot', 'source': 'FRF',
                         'mode': 'cmif', 'caption': ''}])
    bare_html = render_html(bare, project)
    bare_payload = json.loads(bare_html.split(
        'type="application/json">')[1].split('</script>')[0])
    assert 'home_x' not in bare_payload['blocks'][0], (
        'no synthesis, no mode band — the full measurement is the view')


def test_a_flavor_answers_when_the_plain_kind_is_absent():
    """A *bound* never answers for the measurement it bounds — that
    rule stands (see the '@basis:Psd' trap in test_random_report). But
    a flavor of the same measurement is only preferred against, not
    refused: multiple coherence is coherence with the references
    summed over, and the skeleton's one Coherence slot takes either
    kind. Refusing it cost a Rattlesnake test's report its coherence
    figure with no word said — the block resolved to None, the figure
    vanished, and everything after it quietly renumbered.
    """
    from hertzforge.core.data import Coherence, MultipleCoherence
    from hertzforge.core.report import resolve_binding

    def multiple():
        return MultipleCoherence(
            np.linspace(0, 100, 11), np.tile(np.linspace(0, 1, 11), (2, 1)),
            response_dof=['101X+', '102X+'])

    project = {'Run Coherence': multiple()}
    assert resolve_binding('@basis:Coherence', project) == 'Run Coherence'

    # with the plain kind present, it is still preferred — the two are
    # different plots, and asking generically gets the ordinary one
    project['Pair Coherence'] = Coherence(
        np.linspace(0, 100, 11), np.atleast_2d(np.linspace(0, 1, 11)),
        response_dof=['101X+'], reference_dof=['1Z+'])
    assert resolve_binding('@basis:Coherence', project) == 'Pair Coherence'
    # and the specific token never wavers
    assert resolve_binding('@basis:MultipleCoherence',
                           project) == 'Run Coherence'


def test_a_report_is_marked_unclassified_by_default():
    """Work that carries markings is the intended use, and a document
    that says its level outright beats one that says nothing."""
    from hertzforge.core.report import Report

    report = Report('R')
    assert report.marking == 'UNCLASSIFIED'
    assert report.marking_color == 'ink'


def test_the_marking_round_trips(tmp_path):
    import hertzforge
    from hertzforge.core.report import Report

    report = Report('R', marking='SECRET//NOFORN', marking_color='red')
    project = hertzforge.Project('P')
    project.add('Report', report)
    project.save(str(tmp_path / 'p.vdyn'))
    back = hertzforge.Project.open(str(tmp_path / 'p.vdyn'))['Report']
    assert back.marking == 'SECRET//NOFORN'
    assert back.marking_color == 'red'


def test_the_page_carries_one_marking_for_both_banners():
    """One value, two strips: the payload says the marking once, and
    the page draws it top and bottom — banners that can disagree are
    banners that will. The strips are pinned in the JS because the
    canvas half cannot import the field name."""
    import hertzforge
    from hertzforge.core.report import Report
    from hertzforge.report import render_html
    from hertzforge.report.page import _JS

    html = render_html(Report('R', marking='CUI', marking_color='red'),
                       {}, hertzforge.SI)
    assert '"marking": "CUI"' in html
    assert '"marking_color": "red"' in html
    assert "for (const place of ['top', 'bottom'])" in _JS
    assert 'markingStrips' in _JS

    blank = render_html(Report('R', marking=''), {}, hertzforge.SI)
    assert '"marking": ""' in blank, 'blank means unmarked'


def _stage(block):
    """Which of the standing stages a block belongs to, or None for
    prose and results that ride within a stage."""
    kind = block.get('kind')
    source = str(block.get('source', ''))
    if kind == 'scene' and not block.get('shapes'):
        return 'scene'
    if kind == 'table' and 'ChannelTable' in source:
        return 'table'
    if kind == 'bars':
        return 'computed'
    if kind == 'plot':
        # a 3-D stage is classified by what it reads, exactly as its
        # flat counterpart is — the dimension a figure is drawn in
        # says nothing about where it belongs in the argument
        if (block.get('octave') or block.get('reading') == 'ratio'
                or block.get('mode') in ('ratio', 'map', 'mac')
                or 'Coherence' in source):
            return 'computed'
        if 'TimeHistory' in source or 'TransientSpecification' in source:
            return 'time'
        if block.get('floor') or block.get('specification') or any(
                word in source for word in
                ('Psd', 'Specification', 'Srs', 'SineLevel', 'Frf')):
            return 'spectral'
    return None


@pytest.mark.parametrize('build', [
    'modal_template', 'random_template', 'shock_template',
    'sine_template', 'sysid_template', 'transient_template'])
def test_every_template_opens_its_stages_in_the_standing_order(build):
    """Geometry, channel table, time data, spectral data, then the
    computed judgments — the order every report reads in (Brandon,
    2026-08-23). Pinned on where each stage *opens*: a later section
    may re-read spectra (the modal CMIF resynthesis, the octave-band
    comparison) without the skeleton changing shape."""
    from hertzforge.core import report as templates

    blocks = getattr(templates, build)({}).blocks
    stages = [_stage(block) for block in blocks]
    order = ('scene', 'table', 'time', 'spectral', 'computed')
    firsts = []
    for stage in order:
        assert stage in stages, f'{build} has no {stage} stage at all'
        firsts.append(stages.index(stage))
    assert firsts == sorted(firsts), (
        f'{build} opens its stages as {firsts} ({order})')


def _stage_block(objects, name, **extra):
    """The first figure a stage block draws — most tests want one.
    A block can answer with several (`_stage_figures` continues past
    what one stage holds), so unwrap deliberately rather than by
    accident."""
    figures = _stage_figures_of(objects, name, **extra)
    return figures[0] if figures else None


def _stage_figures_of(objects, name, **extra):
    from hertzforge.report import _build_block

    block = {'kind': 'plot', 'mode': 'stage', 'source': name,
             'caption': 'The plant', **extra}
    return _build_block(block, objects, hertzforge.SI, [])


def test_the_stage_payload_is_the_apps_own_stage(project):
    """The report's 3-D figure and the app's 3-D view are one
    normalization (`viz.waterfall.stage_curves`) and one opening view
    (`stage_basis`, derived from `place_camera`) — a figure that
    staged its data differently would be a second picture of one
    measurement (Brandon, 2026-08-23)."""
    import numpy as np

    from hertzforge.drawing.viz.waterfall import STAGE, stage_basis
    from hertzforge.report import MAX_STAGE_RECORDS

    frf = next(n for n, o in project.items()
               if isinstance(o, hertzforge.Frf))
    built = _stage_block(dict(project), frf)
    assert built['kind'] == 'stage'
    # one station per record of the drawn quantity group — one
    # vertical axis holds one quantity, on a stage as on a flat plot
    from hertzforge.drawing.viz.waterfall import waterfall_group
    chosen, _others = waterfall_group(project[frf], None, None)
    assert built['stations'] == min(len(chosen), MAX_STAGE_RECORDS)
    assert len(built['runs']) >= built['stations'], 'one run per record'
    assert built['stage'] == list(STAGE)
    right, up = stage_basis()
    assert np.allclose(built['home'][0], right)
    assert np.allclose(built['home'][1], up)

    # every point stands inside the stage box, and the levels are the
    # stage's own range normalized — the color scale and the vertical
    # axis are one scale, exactly as the app pins its `clim`
    for run in built['runs']:
        assert 0.0 <= run['station'] <= STAGE[1] + 1e-9
        for x, z in run['xz']:
            assert -1e-3 <= x <= STAGE[0] + 1e-3
            assert -1e-3 <= z <= STAGE[2] + 1e-3
        assert min(run['levels']) >= -1e-3
        assert max(run['levels']) <= 1.0 + 1e-3
    x0, x1, z0, z1 = built['extents']
    assert x1 > x0 and z1 > z0, 'the real ranges the box stands in for'
    assert built['xlabel'] and built['zlabel']


def test_a_block_continues_into_the_figures_it_needs(project):
    """No cap on the figure count (Brandon, 2026-08-24): an object
    with more channels than one stage holds legibly continues into
    the next figure, and every record lands in one of them. The
    continuations are the app's own pages."""
    from hertzforge.report import MAX_STAGE_RECORDS

    frf = next(n for n, o in project.items()
               if isinstance(o, hertzforge.Frf))
    figures = _stage_figures_of(dict(project), frf)
    held = project[frf].num_records
    assert len(figures) == -(-held // MAX_STAGE_RECORDS) == 3
    drawn = sum(len(f['labels']) for f in figures)
    assert drawn == held, 'every record is in some figure'
    assert all(len(f['labels']) <= MAX_STAGE_RECORDS for f in figures)
    # each says which stretch it holds, or a reader cannot tell one
    # continuation from the next
    assert 'records 1 to 48 of 100' in figures[0]['caption']
    assert 'records 49 to 96 of 100' in figures[1]['caption']
    assert 'records 97 to 100 of 100' in figures[2]['caption']
    # and the records themselves do not repeat between figures
    everywhere = [label for f in figures for label in f['labels']]
    assert len(set(everywhere)) == len(everywhere) == held


def test_the_budget_bounds_the_file_not_the_figure_count():
    """The bound is bytes, not taste: a figure is ~736 KB of geometry
    inside a self-contained file, so a 25,380-record stress set would
    ask for 529 of them and a document nothing can open. What the
    budget leaves out is said, never dropped in silence."""
    import numpy as np

    from hertzforge.core.data import TimeHistory
    from hertzforge.report import (
        MAX_STAGE_RECORDS,
        MAX_STAGE_TOTAL,
        _build_block,
    )

    over = MAX_STAGE_TOTAL + 3 * MAX_STAGE_RECORDS
    t = np.arange(64) / 64.0
    data = TimeHistory(t, np.ones((over, len(t))),
                       response_dof=[f'{1000 + i}Z+' for i in range(over)],
                       ordinate_dim='acceleration',
                       ordinate_unit='m/s**2')
    figures = _build_block({'kind': 'plot', 'mode': 'stage',
                            'source': 'T', 'caption': 'Many'},
                           {'T': data}, hertzforge.SI, [])
    drawn = sum(len(f['labels']) for f in figures)
    assert drawn >= MAX_STAGE_TOTAL, 'it draws right up to the budget'
    assert drawn < over, 'and stops there'
    assert len(figures) == -(-drawn // MAX_STAGE_RECORDS)
    assert f'{over - drawn} further records are not drawn' in \
        figures[-1]['caption'], figures[-1]['caption']


def test_the_stage_is_bounded_for_a_document(project):
    """A screen is driven and pages; a page in a document is not. The
    caps are the figure's bound, and records past them are said in the
    caption rather than dropped in silence."""
    from hertzforge.report import MAX_FIGURE_POINTS, MAX_STAGE_RECORDS

    frf = next(n for n, o in project.items()
               if isinstance(o, hertzforge.Frf))
    built = _stage_block(dict(project), frf)
    assert len(built['runs']) <= MAX_STAGE_RECORDS
    for run in built['runs']:
        # the figure's points are its budget shared out, so a set of
        # a few records draws them whole — a report should not reduce
        # the data it reports (Brandon, 2026-08-24)
        assert len(run['xz']) <= MAX_FIGURE_POINTS
        assert len(run['levels']) == len(run['xz'])

    many = project[frf].num_records > MAX_STAGE_RECORDS
    assert ('of' in built['caption']) == many, \
        'a capped figure says so; an uncapped one says nothing'


def test_the_stage_filters_like_every_other_figure(project):
    """`select` reads the same way on a stage as on a flat plot — the
    drive points, or one quantity."""
    frf = next(n for n, o in project.items()
               if isinstance(o, hertzforge.Frf))
    objects = dict(project)
    whole = _stage_block(objects, frf)
    drives = _stage_block(objects, frf, select='drive')
    assert drives is not None
    assert drives['stations'] < whole['stations']
    assert _stage_block(objects, frf, select='dim:temperature') is None, \
        'a filter that keeps nothing has nothing to show'


def test_a_paired_stage_stands_the_quieter_object_back():
    """Two objects on one stage, met station by station: the louder
    colored by level, the quieter stood back off the color scale —
    the app's own overlay reading, in the document (Brandon,
    2026-08-23)."""
    import numpy as np

    from hertzforge.core.data import Psd
    from hertzforge.report import _build_block

    f = np.arange(1.0, 33.0)
    rows = np.outer([1.0, 2.0], np.ones(len(f)))

    def densities(scale):
        return Psd(f, rows * scale, response_dof=['101Z+', '104Z+'],
                   ordinate_dim='acceleration**2/frequency',
                   ordinate_unit='m/s**2')

    objects = {'Noise': densities(1e-4), 'Excitation': densities(1.0)}
    built = _build_block(
        {'kind': 'plot', 'mode': 'stage', 'source': 'Excitation',
         'floor': 'Noise', 'caption': 'Pair'},
        objects, hertzforge.SI, [])[0]
    assert built['kind'] == 'stage'
    loud = [r for r in built['runs'] if not r.get('quiet')]
    quiet = [r for r in built['runs'] if r.get('quiet')]
    assert len(loud) == len(quiet) == 2, 'both objects, both channels'
    assert all('levels' in r for r in loud), 'the louder is on the scale'
    assert all('levels' not in r for r in quiet), \
        'the quieter is off it — gray, never competing'
    for a, b in zip(loud, quiet):
        assert a['station'] == b['station'], 'the pair meets at one station'
    assert built['labels'][0] in ('101Z+', '101Z+ acceleration'), \
        'stations wear the record label'


def test_the_ratio_reads_in_decibels_on_the_stage():
    """The divided reading: one curve per channel, linear in dB — the
    number being read *is* the decibel — through the same
    `snr.signal_to_noise` the flat figure divides by."""
    import numpy as np

    from hertzforge.core.data import Psd
    from hertzforge.report import _build_block

    f = np.arange(1.0, 33.0)

    def densities(scale):
        return Psd(f, np.ones((2, len(f))) * scale,
                   response_dof=['101Z+', '104Z+'],
                   ordinate_dim='acceleration**2/frequency',
                   ordinate_unit='m/s**2')

    objects = {'Noise': densities(1e-3), 'Excitation': densities(1.0)}
    built = _build_block(
        {'kind': 'plot', 'mode': 'stage', 'source': 'Excitation',
         'floor': 'Noise', 'reading': 'ratio', 'caption': 'SNR'},
        objects, hertzforge.SI, [])[0]
    assert built['zlabel'] == 'signal to noise [dB]'
    assert built['log'] is False, 'a decibel is already the log reading'
    assert not any(r.get('quiet') for r in built['runs']), \
        'a ratio has one curve per station, not two'
    z0, z1 = built['extents'][2], built['extents'][3]
    assert 29 < z0 <= z1 < 31, f'a planted 30 dB ratio reads as one: {z0}'


def test_a_dimensionless_axis_names_what_it_measures():
    """A coherence's unit is the bare '1' — true, and it tells a
    reader nothing. With no unit worth printing the axis names the
    measurement instead (Brandon's coherence stage came out titled
    '1')."""
    import numpy as np

    from hertzforge.core.data import MultipleCoherence
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    f = np.arange(1.0, 17.0)
    coherence = MultipleCoherence(f, np.ones((2, len(f))) * 0.9,
                                  response_dof=['101Z+', '104Z+'])
    arrays = waterfall_arrays(coherence, None, hertzforge.SI,
                              budget=32)
    assert arrays['zlabel'] == 'multiple coherence'


def test_a_time_history_stage_carries_its_analysis_marks():
    """A trace in a report is read for what part of the run was
    analyzed and how — the reason the flat figure carries its frames
    (Brandon, 2026-08-23). The stage carries the same marks: the
    analyzed span, and the rail of window weights on the back wall,
    from `viz.marks`' own geometry so the two views cannot disagree
    about where a frame is."""
    import numpy as np

    from hertzforge.core.averaging import Averaging
    from hertzforge.core.data import TimeHistory
    from hertzforge.drawing.viz.marks import averaging_stage_geometry
    from hertzforge.report import _build_block

    rate = 256.0
    t = np.arange(int(rate * 4)) / rate
    history = TimeHistory(t, np.ones((2, len(t))),
                          response_dof=['101Z+', '104Z+'],
                          ordinate_dim='acceleration',
                          ordinate_unit='m/s**2')
    history.averaging = Averaging(frame_length=256, frames=3,
                                  overlap=0.5, window='hann', start=0.5)
    built = _build_block(
        {'kind': 'plot', 'mode': 'stage', 'source': 'T',
         'caption': 'Trace'}, {'T': history}, hertzforge.SI, [])[0]
    marks = built['marks']['averaging']
    assert len(marks['frames']) == 3, 'one rail slot per frame'
    assert marks['label'].startswith('3 x 256 samples, hann')
    # the very numbers the app's stage draws, for the same averaging
    # over the same extents
    same = averaging_stage_geometry(history.averaging, rate,
                                    built['extents'])
    assert marks['span'] == same['span']
    assert marks['frames'][0]['glyph'] == same['frames'][0]['glyph']
    # frames that overlap cannot share a rail slot without their
    # windows running through each other, so the baselines step —
    # one level per `Averaging.levels`, which is the rule the 2-D
    # rail follows too
    levels, count = history.averaging.levels(rate)
    baselines = [frame['baseline'] for frame in marks['frames']]
    assert count == 2, 'half overlap: two slots'
    assert len(set(baselines)) == count, \
        f'{count} slots, {len(set(baselines))} heights: {baselines}'
    assert baselines[0] != baselines[1], \
        'consecutive frames overlap, so they step apart'
    assert [sorted(set(baselines)).index(b) for b in baselines] == \
        list(levels), 'each frame on the slot its level names'
    # a hann's weights run 0 -> 1 -> 0, which is what the rail draws
    weights = marks['frames'][0]['weights']
    assert max(weights) == pytest.approx(1.0, abs=1e-3)
    assert weights[0] == pytest.approx(0.0, abs=1e-3)
    assert len(marks['frames'][0]['caps']) == 2, 'a tick at each end'
    assert 'shocks' not in built['marks'], 'nothing said no shocks'


def test_a_shock_stage_carries_its_numbered_windows():
    """A shock trace's whole character is which stretches were
    analyzed; the slabs say so, and the numbers are what 'shock 2'
    points at."""
    import numpy as np

    from hertzforge.core.data import TimeHistory
    from hertzforge.core.shocks import Shock
    from hertzforge.report import _build_block

    rate = 512.0
    t = np.arange(int(rate * 3)) / rate
    history = TimeHistory(t, np.ones((2, len(t))),
                          response_dof=['101Z+', '104Z+'],
                          ordinate_dim='acceleration',
                          ordinate_unit='m/s**2')
    history.shocks = (Shock(0.5, 0.2), Shock(1.5, 0.2))
    built = _build_block(
        {'kind': 'plot', 'mode': 'stage', 'source': 'T',
         'caption': 'Shocks'}, {'T': history}, hertzforge.SI, [])[0]
    windows = built['marks']['shocks']['windows']
    assert [w['label'] for w in windows] == ['1', '2']
    x0, x1 = built['extents'][0], built['extents'][1]
    stage_x = built['stage'][0]
    # the first window opens half a second into a three-second record
    def seconds(at):
        return at / stage_x * (x1 - x0) + x0

    opens = seconds(windows[0]['span'][0])
    assert opens == pytest.approx(0.5, abs=1e-3)
    # and it is as wide as the event is long — a slab with no width
    # marks a moment where the analysis covered a stretch
    assert seconds(windows[0]['span'][1]) - opens == \
        pytest.approx(0.2, abs=1e-3)
    assert seconds(windows[1]['span'][0]) == pytest.approx(1.5, abs=1e-3)
    assert 'averaging' not in built['marks'], 'this one was never framed'


TEMPLATES = ('modal', 'random', 'shock', 'sine', 'sysid', 'transient')


@pytest.mark.parametrize('build', TEMPLATES)
def test_no_report_asks_the_reader_to_finish_it(build):
    """Nothing is left for the user to fill in later (Brandon,
    2026-08-24): a report is complete from what the project holds, and
    where a number is wanted it is a live reference that fills itself
    in rather than a blank."""
    from hertzforge.core import report as templates

    blocks = getattr(templates, f'{build}_template')({}).blocks
    prose = '\n'.join(b.get('text', '') for b in blocks
                      if b.get('kind') == 'text')
    for placeholder in ('fill this in', 'go here', 'add observations',
                        'add the test', 'tbd', 'to be completed',
                        '*add', 'insert '):
        assert placeholder not in prose.lower(), \
            f'{build} still asks the reader to write something'


@pytest.mark.parametrize('build', TEMPLATES)
def test_every_figure_is_explained_somewhere(build):
    """Each figure gets a generic explanation of what it is and why it
    is shown (Brandon, 2026-08-24). Pinned by the prose naming it: a
    figure nobody refers to is one a reader has to guess at."""
    from hertzforge.core import report as templates

    blocks = getattr(templates, f'{build}_template')({}).blocks
    prose = ' '.join(b.get('text', '') for b in blocks
                     if b.get('kind') == 'text').lower()
    def words_of(text):
        cleaned = ''.join(c if c.isalnum() or c.isspace() else ' '
                          for c in text.lower())
        return cleaned.split()

    said = ' '.join(words_of(prose))
    unexplained = []
    for block in blocks:
        caption = block.get('caption', '')
        if block.get('kind') == 'text' or not caption:
            continue
        # a reference names the start of a caption, so the prose is
        # searched for any leading run of it — down to a single word,
        # since 'Instrumentation' is a whole caption
        words = words_of(caption)
        if not any(' '.join(words[:n]) in said
                   for n in range(len(words), 0, -1)):
            unexplained.append(caption[:60])
    assert not unexplained, f'{build} draws unexplained figures: {unexplained}'


def test_a_report_does_not_reduce_the_data_it_reports():
    """Brandon zoomed into the shock traces and found them thinned
    (2026-08-24). A figure whose data has been reduced cannot answer a
    question asked of it later, however faithful the reduction looked
    at the opening view — so the budget is set where the data itself
    usually fits, and a record of ordinary length draws sample for
    sample."""
    import numpy as np

    from hertzforge.core.data import TimeHistory
    from hertzforge.report import MAX_FIGURE_POINTS, _build_block

    rate, seconds, channels = 4096.0, 6.0, 6
    n = int(rate * seconds)
    rng = np.random.default_rng(21)
    data = TimeHistory(
        np.arange(n) / rate, rng.standard_normal((channels, n)),
        response_dof=[f'{101 + i}Z+' for i in range(channels)],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')
    assert channels * n < MAX_FIGURE_POINTS, 'the fixture fits the budget'

    figures = _build_block({'kind': 'plot', 'mode': 'stage',
                            'source': 'T', 'caption': 'Traces'},
                           {'T': data}, hertzforge.SI, [])
    (figure,) = figures
    assert all(len(run['xz']) == n for run in figure['runs']), \
        'every sample of every record is in the figure'
    assert 'note' not in figure, 'nothing was thinned, so nothing is said'

    # and past the budget it thins, keeps every extreme, and says so
    wide = TimeHistory(
        np.arange(n) / rate,
        rng.standard_normal((channels, n)) * 1.0,
        response_dof=[f'{201 + i}Z+' for i in range(channels)],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')
    import hertzforge.report as report_module
    was = report_module.MAX_FIGURE_POINTS
    try:
        report_module.MAX_FIGURE_POINTS = 6000
        (figure,) = _build_block({'kind': 'plot', 'mode': 'stage',
                                  'source': 'T', 'caption': 'Traces'},
                                 {'T': wide}, hertzforge.SI, [])
    finally:
        report_module.MAX_FIGURE_POINTS = was
    assert all(len(run['xz']) < n for run in figure['runs'])
    assert 'every extreme kept' in figure['note']


def test_every_thinned_figure_says_so_and_no_other_one_does():
    """"Any plot where the data gets decimated should be clearly marked
    as such" (Brandon, 2026-08-24).

    Five paths in the builder can reduce what they draw, and each is
    checked here against a budget small enough to bite: the 3-D stage,
    the flat curve, the flat pair, and the coherence map. The fifth
    case is the one that makes this test worth its length — a **stepped
    spectrum is never thinned at all**, because cutting a staircase
    destroys the treads and the area beneath them, and it draws two
    points a line besides. Reading the mark off the *budget* rather
    than off the drawing had it announce "512 points drawn of 1025" for
    a spectrum drawn whole in 2050, which is a worse failure than a
    missing mark: a reader who is told the data was reduced stops
    trusting a figure that is in fact exact.
    """
    import numpy as np

    import hertzforge.report as report_module
    from hertzforge.core.data import MultipleCoherence, Psd, TimeHistory
    from hertzforge.report import _build_block

    rate, n, channels = 4096.0, 8192, 4
    rng = np.random.default_rng(5)
    history = TimeHistory(
        np.arange(n) / rate, rng.standard_normal((channels, n)),
        response_dof=[f'{101 + i}Z+' for i in range(channels)],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')
    lines = 1024
    psd = Psd(np.arange(lines) * (rate / 2 / lines),
              rng.random((channels, lines)) + 0.1,
              response_dof=[f'{101 + i}Z+' for i in range(channels)],
              ordinate_dim='acceleration', ordinate_unit='(m/s**2)**2/Hz')
    objects = {'T': history, 'P': psd}

    def note(block, **budgets):
        held = {name: getattr(report_module, name) for name in budgets}
        try:
            for name, value in budgets.items():
                setattr(report_module, name, value)
            built = _build_block({'caption': 'c', **block}, objects,
                                 hertzforge.SI, [])
        finally:
            for name, value in held.items():
                setattr(report_module, name, value)
        figure = built[0] if isinstance(built, list) else built
        return (figure or {}).get('note', '')

    tight = {'MAX_FIGURE_POINTS': 4000, 'MAX_POINTS_PER_CURVE': 200}
    stage = {'kind': 'plot', 'mode': 'stage', 'source': 'T'}
    assert 'Thinned' in note(stage, **tight)
    assert 'Thinned' not in note(stage), 'it fits, so nothing is claimed'

    flat = {'kind': 'plot', 'source': 'T'}
    assert 'Thinned' in note(flat, **tight)
    assert 'Thinned' not in note(flat)

    # the false claim, pinned: a stepped spectrum draws whole however
    # small the budget, so it must stay silent at a budget under its
    # own line count
    stepped = {'kind': 'plot', 'mode': 'stage', 'source': 'P'}
    assert psd.interpolation == 'bin', 'the fixture is the stepped kind'
    assert 'Thinned' not in note(stepped, MAX_FIGURE_POINTS=4000), (
        'a staircase is never thinned — saying otherwise tells a reader '
        'to distrust an exact figure')

    # the flat pair, which thins both sides of the comparison
    objects['Q'] = Psd(psd.abscissa, psd.ordinate * 0.01,
                       response_dof=list(psd.response_dof),
                       ordinate_dim='acceleration',
                       ordinate_unit='(m/s**2)**2/Hz')
    pair = {'kind': 'plot', 'mode': 'pair', 'source': 'P', 'floor': 'Q'}
    assert 'Thinned' in note(pair, MAX_POINTS_PER_CURVE=200)
    assert 'Thinned' not in note(pair)

    # and the map, which strides frequency to fit its columns — a
    # different reduction, so it names the stride rather than a count
    objects['C'] = MultipleCoherence(
        psd.abscissa, rng.random((channels, lines)),
        response_dof=list(psd.response_dof))
    colors = {'kind': 'plot', 'mode': 'map', 'source': 'C'}
    assert ('every 25th frequency line drawn of 1024'
            in note(colors, MAX_MAP_COLUMNS=40))
    assert 'Thinned' not in note(colors)


def test_captions_carry_live_references_and_leave_figure_refs_alone():
    """A filtered-transients figure names its corner in the caption,
    and the number must follow the setting the way the prose does
    (Brandon, 2026-08-24 — the first render shipped a literal
    '{{Time Data.filter_corner}}' under Figure 2 — the field has
    since become filter_description). Field references
    only: a figure reference has no dot, so it stays for the markdown
    pass, where every label exists."""
    import numpy as np

    from hertzforge.core.data import TimeHistory
    from hertzforge.core.filters import Filtering
    from hertzforge.report import _build_block

    data = TimeHistory(np.arange(256) / 256.0,
                       np.random.default_rng(2).standard_normal((1, 256)),
                       response_dof='101Z+', ordinate_dim='acceleration')
    data.filtering = Filtering(high=40.0)
    built = _build_block(
        {'kind': 'plot', 'source': 'T', 'mode': 'stage',
         'caption': 'Filtered ({{T.filter_description}}), '
                    'see {{figure:Overview}}'},
        {'T': data}, hertzforge.SI, [])
    figure = built[0] if isinstance(built, list) else built
    assert figure['caption'] == \
        'Filtered (low-pass at 40 Hz), see {{figure:Overview}}'


def test_the_front_matter_draws_a_dof_scene_per_quantity_the_run_holds():
    """Brandon, 2026-09-08: every stress project's report carried one
    empty card, 'Excitation degrees of freedom (voltage)' — the runs
    excite through a load cell and hold no voltage channel. A scene is
    emitted for the quantities the source measures; every quantity when
    there is no source to ask, so a bare project keeps its slots."""
    from hertzforge.core.data import TimeHistory
    from hertzforge.core.report import front_matter

    t = np.linspace(0, 1, 64)
    time = TimeHistory(t, np.ones((2, 64)), response_dof=['101Z+', '110Z+'],
                       ordinate_dim=['acceleration', 'force'])
    scenes = [b for b in front_matter({'Time': time}, None, 'Time')
              if b.get('dofs')]
    assert [b['dofs'] for b in scenes] == ['force', 'acceleration'], \
        'excitation first, and no voltage scene to leave empty'
    assert scenes[0]['caption'].startswith('Excitation')
    bare = [b for b in front_matter({}, None, '@basis:TimeHistory')
            if b.get('dofs')]
    assert [b['dofs'] for b in bare] == ['voltage', 'force', 'acceleration'], \
        'nothing to ask: the slots stay'


def test_the_scalogram_figure_says_when_it_thinned_time():
    """The figure holds time to `SCALOGRAM_COLUMNS` by peak-hold — the
    reading the app draws from — and says so in its note, with the
    count it kept and the count it had; a record that fits is drawn
    whole and carries no note."""
    from hertzforge.core.data import TimeHistory
    from hertzforge.report import SCALOGRAM_COLUMNS

    def figure(count):
        fs = 1024.0
        t = np.arange(count) / fs
        history = TimeHistory(
            t, np.sin(2 * np.pi * 100.0 * t)[None, :],
            response_dof=['101Z+'], ordinate_dim='acceleration')
        report = Report('R', [
            {'kind': 'plot', 'source': 'Time History', 'mode': 'scalogram',
             'select': 'dim:acceleration', 'caption': 'Scalogram'}])
        payload = json.loads(render_html(
            report, {'Time History': history}).split(
            'type="application/json">')[1].split('</script>')[0])
        return payload['blocks'][0]

    long = figure(8192)
    step = -(-8192 // SCALOGRAM_COLUMNS)
    kept = -(-8192 // step)
    assert kept < 8192
    assert long['note'] == (f'time thinned to {kept} columns of 8192, '
                            f'keeping each bin\'s peak')
    assert len(long['sheet']['xs']) == kept
    assert all(len(row) == kept for row in long['sheet']['levels'])

    short = figure(200)
    assert 'note' not in short
    assert len(short['sheet']['xs']) == 200
