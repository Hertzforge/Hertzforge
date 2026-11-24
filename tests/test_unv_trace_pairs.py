"""A UNV file's labeled time-trace pairs: each measurement's response and
reference traces become one time history, with their roles stated."""

import re

import numpy as np
import pytest

from hertzforge import Project, TimeHistory
from hertzforge.io import unv

T = np.arange(8) / 8


def values(node):
    return np.array([np.arange(8.0), -np.arange(8.0)]) + node


def trace_text(tmp_path, nodes=(730, 1125), *, units=True, reference='1Z+',
               labels=('Response Time Trace', 'Reference Time Trace'), drop=None):
    """An export of one measurement per node, each written the way such an
    exporter writes it: two time traces that both carry the measurement's
    DOF as their response and the reference in the reference field, one
    labeled as the response trace and one as the reference. `drop` leaves
    out one trace, as (node, label index)."""
    blocks = []
    for node in nodes:
        history = TimeHistory(T, values(node), response_dof=[f'{node}Z-'] * 2,
                              reference_dof=[reference] * 2,
                              ordinate_dim=['velocity', 'force'])
        path = tmp_path / 'one.unv'
        unv.save(history, path)
        text = path.read_text()
        head, _, rest = text.partition('    -1\n    -1\n    58\n')
        records = ('    58\n' + rest).split('    -1\n    -1\n')
        if units and not blocks:
            blocks.append(head + '    -1\n')
        for which, record in enumerate(records):
            if drop == (node, which):
                continue
            record = record.removesuffix('    -1\n')
            label = record.split('\n')[1]
            record = record.replace(label, labels[which], 1)
            blocks.append('    -1\n' + record + '    -1\n')
    path = tmp_path / 'traces.unv'
    path.write_text(''.join(blocks))
    return path


@pytest.mark.parametrize('units', [True, False])
def test_each_measurement_is_one_history_with_its_reference_beside_it(tmp_path, units):
    path = trace_text(tmp_path, units=units)
    project = Project()
    names = project.import_file(path, reference_node=1013)
    assert names == ['Time History - 730Z- - ref 1013Z+',
                     'Time History - 1125Z- - ref 1013Z+']
    for node, name in zip((730, 1125), names):
        data = project[name]
        assert data.response_dof == [f'{node}Z-', '1013Z+']
        assert data.reference_dof is None
        quantities = (['velocity', 'force'] if units else ['unknown', 'unknown'])
        assert data.ordinate_dim == quantities
        assert data.roles == {(f'{node}Z-', quantities[0]): 'response',
                              ('1013Z+', quantities[1]): 'reference'}
        assert data.channel_roles() == data.roles
        np.testing.assert_allclose(data.ordinate, values(node))
        np.testing.assert_allclose(data.abscissa, T)
        assert data.units_defined == units
        if not units:
            assert data.dimension_hint == ['velocity', 'force']
    # one file, one group, so the file's histories are compared as a set
    [group] = project.object_groups
    assert group['name'] == 'traces.unv' and group['members'] == names


def test_a_test_file_and_a_model_file_are_the_two_sets_a_comparison_takes(tmp_path):
    (tmp_path / 'test').mkdir()
    (tmp_path / 'model').mkdir()
    test = trace_text(tmp_path / 'test', nodes=(730, 1125))
    model = trace_text(tmp_path / 'model', nodes=(730, 1125))
    project = Project()
    tested = project.import_file(test, reference_node=1013)
    modeled = project.import_file(model, reference_node=1013)
    assert len(project.object_groups) == 2
    assert project.time_history_sets(*tested, *modeled) == (tested, modeled)
    name = project.compare_time_histories(*tested, *modeled)
    # the responses are paired station by station; the references are left out
    assert project[name].response_dof == ['730Z-', '730Z-', '1125Z-', '1125Z-']


def test_the_roles_and_names_survive_a_save(tmp_path):
    project = Project()
    names = project.import_file(trace_text(tmp_path), reference_node=1013)
    saved = tmp_path / 'saved.hforge'
    project.save(saved)
    restored = Project.open(saved)
    assert list(restored) == names
    assert restored[names[0]].roles == project[names[0]].roles


def test_without_a_reference_node_the_files_own_stands(tmp_path):
    out = unv.load(trace_text(tmp_path))
    assert list(out) == ['time - 730Z- - ref 1Z+', 'time - 1125Z- - ref 1Z+']
    assert out['time - 730Z- - ref 1Z+'].response_dof == ['730Z-', '1Z+']


def test_the_reference_node_keeps_the_files_direction(tmp_path):
    data = next(iter(unv.load(trace_text(tmp_path, reference='4Y-'),
                              reference_node=22).values()))
    assert data.response_dof == ['730Z-', '22Y-']


def test_the_reference_dofs_a_file_names_are_listed(tmp_path):
    assert unv.trace_pair_references(trace_text(tmp_path)) == ['1Z+']
    history = TimeHistory(T, np.ones((2, 8)), response_dof=['1Z+', '2Z+'],
                          ordinate_dim='velocity')
    plain = tmp_path / 'plain.unv'
    unv.save(history, plain)
    assert unv.trace_pair_references(plain) == []


def test_ordinary_time_records_stay_one_history(tmp_path):
    history = TimeHistory(T, np.ones((2, 8)), response_dof=['1Z+', '2Z+'],
                          reference_dof=['3Z+', '3Z+'], ordinate_dim='velocity')
    path = tmp_path / 'plain.unv'
    unv.save(history, path)
    read = unv.load(path)
    assert isinstance(read, TimeHistory) and read.num_records == 2
    assert read.roles is None
    project = Project()
    assert project.import_file(path) == ['Time History']
    assert project.object_groups == []


@pytest.mark.parametrize('drop, said', [
    ((1125, 1), ('the Response Time Trace at 1125Z- (function record 3) has no '
                'Reference Time Trace')),
    ((730, 0), ('the Reference Time Trace at 730Z- (function record 1) has no '
               'Response Time Trace')),
])
def test_a_trace_without_its_partner_is_refused_by_name(tmp_path, drop, said):
    with pytest.raises(ValueError, match='^' + re.escape(said)):
        unv.load(trace_text(tmp_path, drop=drop))


def test_a_trace_with_no_reference_dof_is_refused_by_name(tmp_path):
    path = trace_text(tmp_path, nodes=(730,))
    text = path.read_text().replace('NONE               1   3', 'NONE               0   0')
    path.write_text(text)
    with pytest.raises(ValueError, match=r'^the Response Time Trace at 730Z- '
                                         r'\(function record 1\) names no reference DOF'):
        unv.load(path)


@pytest.mark.parametrize('node', [0, -3, 2.5, True, '7'])
def test_a_reference_node_that_is_not_a_node_is_refused(tmp_path, node):
    with pytest.raises(ValueError, match='reference_node is the node the reference'):
        unv.load(trace_text(tmp_path), reference_node=node)


def test_a_measurement_exported_twice_gives_two_histories(tmp_path):
    out = unv.load(trace_text(tmp_path, nodes=(730, 730)))
    assert list(out) == ['time - 730Z- - ref 1Z+', 'time - 730Z- - ref 1Z+ (2)']


def test_a_reference_node_is_refused_where_it_would_do_nothing_or_merge(tmp_path):
    history = TimeHistory(T, np.ones((2, 8)), response_dof=['1Z+', '2Z+'],
                          ordinate_dim='velocity')
    plain = tmp_path / 'plain.unv'
    unv.save(history, plain)
    with pytest.raises(ValueError, match='^plain.unv has no labeled time-trace pairs'):
        unv.load(plain, reference_node=4)
    two = trace_text(tmp_path, nodes=(730,))
    (tmp_path / 'b').mkdir()
    other = trace_text(tmp_path / 'b', nodes=(1125,), reference='2Z+')
    two.write_text(two.read_text() + other.read_text())
    with pytest.raises(ValueError, match="time-trace pairs name references at nodes 1, 2"):
        unv.load(two, reference_node=4)
    # read as the file gives them, the two references stay apart, and the
    # import asks nothing: one answer could not be right for both
    assert list(unv.load(two)) == ['time - 730Z- - ref 1Z+', 'time - 1125Z- - ref 2Z+']
    from hertzforge.service import projects

    assert projects.import_question({'path': str(two)}) is None


# ---- the question the service asks ---------------------------------------------

def test_the_service_asks_for_the_node_and_shows_the_files_reference(tmp_path):
    from hertzforge.service import projects

    path = str(trace_text(tmp_path))
    ask = projects.import_question({'path': path})
    assert ask['param'] == 'reference_node' and ask['value'] == 1
    assert 'traces.unv' in ask['question'] and '1Z+' in ask['question']
    # answered, it asks nothing more and the answer is what is read
    assert projects.import_question({'path': path, 'reference_node': 1013}) is None
    state, added = projects.import_file(None, {'path': path, 'reference_node': 1013})
    assert added == ['Time History - 730Z- - ref 1013Z+', 'Time History - 1125Z- - ref 1013Z+']
    assert state.project.journal[-1] == f'project.import_file({path!r}, reference_node=1013)'


def test_a_file_without_labeled_pairs_asks_nothing(tmp_path):
    from hertzforge.service import projects

    history = TimeHistory(T, np.ones((2, 8)), response_dof=['1Z+', '2Z+'],
                          ordinate_dim='velocity')
    plain = tmp_path / 'plain.unv'
    unv.save(history, plain)
    assert projects.import_question({'path': str(plain)}) is None


@pytest.mark.parametrize('node', [0, 2.5, True, '7'])
def test_the_service_refuses_an_answer_that_is_not_a_node(tmp_path, node):
    from hertzforge.service import projects
    from hertzforge.service.protocol import ProtocolError

    with pytest.raises(ProtocolError, match='positive whole number') as error:
        projects.import_file(None, {'path': str(trace_text(tmp_path)), 'reference_node': node})
    assert error.value.code == 'bad_request'


def test_the_service_refuses_a_reference_node_for_another_kind_of_file(tmp_path):
    from hertzforge.service import projects
    from hertzforge.service.protocol import ProtocolError

    path = tmp_path / 'table.csv'
    path.write_text('1,2\n')
    with pytest.raises(ProtocolError, match="only a UNV file's time-trace pairs"):
        projects.import_file(None, {'path': str(path), 'reference_node': 3})


async def _exchange(requests):
    from websockets.asyncio.client import connect

    from hertzforge.service import frames
    from hertzforge.service.server import Service, start

    service = Service()
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    out = []
    try:
        async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
            frames.decode(await socket.recv())
            for ident, (method, params) in enumerate(requests, 1):
                await socket.send(frames.encode_text(
                    {'type': 'request', 'id': ident, 'method': method, 'params': params}))
                while True:
                    message = frames.decode(await socket.recv())
                    if message['type'] == 'response':
                        out.append(message)
                        break
    finally:
        server.close()
        await server.wait_closed()
        service.close()
    return out


def test_over_the_socket_the_import_asks_first_and_reads_the_answer(tmp_path):
    import asyncio
    import json
    from pathlib import Path

    import jsonschema

    schema = json.loads((Path(__file__).resolve().parents[1] / 'schema'
                         / 'protocol.schema.json').read_text())

    def check(instance, name):
        jsonschema.Draft202012Validator({'$schema': schema['$schema'], '$defs': schema['$defs'],
                                         '$ref': f'#/$defs/{name}'}).validate(instance)

    path = str(trace_text(tmp_path))
    asked, answered = asyncio.run(_exchange([
        ('project.import', {'path': path}),
        ('project.import', {'path': path, 'reference_node': 1013}),
    ]))
    first = asked['result']
    check(first, 'projectImportResult')
    check({'path': path, 'reference_node': 1013}, 'projectImportParams')
    # asked, nothing is read: the project the shell is shown is still empty
    assert first['added'] == [] and first['project']['objects'] == []
    assert first['ask']['param'] == 'reference_node' and first['ask']['value'] == 1
    second = answered['result']
    check(second, 'projectImportResult')
    assert 'ask' not in second
    assert second['added'] == ['Time History - 730Z- - ref 1013Z+',
                               'Time History - 1125Z- - ref 1013Z+']
    assert all(o['groups'] == ['traces.unv'] for o in second['project']['objects'])
