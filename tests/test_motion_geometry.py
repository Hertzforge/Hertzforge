"""Sensors placed and turned by the rigid motion they measured
(`core.motion_geometry`), against layouts made here with numpy alone."""

import itertools

import numpy as np
import pytest
from scipy.spatial.transform import Rotation

from hertzforge import Geometry, Project
from hertzforge.core import motion_geometry as mg
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory

RATE = 1024.0
SECONDS = 24.0
FRAMES = Averaging(frame_length=1024, overlap=0.5, window='hann', frames=46)
#: the sensors' axes: five of three, three of two and three of one
KINDS = ('XYZ', 'XYZ', 'XYZ', 'XYZ', 'XYZ', 'XY', 'YZ', 'XZ', 'Z', 'X', 'Y')


class Layout:
    """Sensors at random places, each turned at random, on a body that
    moves in six rigid motions of unequal strength and bends in two modes
    well above the band, with white noise on every channel."""

    def __init__(self, kinds=KINDS, seed=0, signs=None):
        rng = np.random.default_rng(seed)
        self.nodes = [10 * (k + 1) for k in range(len(kinds))]
        self.kinds = dict(zip(self.nodes, kinds, strict=True))
        self.position = {node: rng.uniform(-0.3, 0.3, 3) for node in self.nodes}
        self.frame = {node: Rotation.random(random_state=rng).as_matrix().T
                      for node in self.nodes}
        self.signs = signs or {}
        self.dofs = [f'{node}{axis}{self.signs.get((node, axis), "+")}'
                     for node in self.nodes for axis in self.kinds[node]]
        self.rng = rng

    def lines(self, mirrored=False):
        """[d, r x d] per channel: what each reads of the six rigid motions."""
        rows = []
        for node in self.nodes:
            for axis in self.kinds[node]:
                d = self.frame[node]['XYZ'.index(axis)]
                d = -d if self.signs.get((node, axis)) == '-' else d
                where = -self.position[node] if mirrored else self.position[node]
                rows.append(np.concatenate([d, np.cross(where, d)]))
        return np.array(rows)

    def history(self, motions=(1.0, 0.8, 0.6, 3.0, 2.5, 2.0), noise=1e-3, bending=1.0,
                start=0.0, seed=1):
        """A time history of accelerations, m/s²: eight random forces move
        the body in its six rigid motions, at the levels `motions` (three
        translations, three rotations), and bend it in two modes at 310
        and 420 Hz, as hard; noise is added. The forces that move it are
        the ones that bend it, as on a structure, so the bending in the
        low band is coherent with the rigid motion."""
        rng = np.random.default_rng(seed)
        count = int(SECONDS * RATE)
        lines = self.lines()
        freq = np.fft.rfftfreq(count, 1 / RATE)
        forces = np.fft.rfft(rng.standard_normal((8, count)), axis=1)
        mix = rng.standard_normal((6, 8)) / np.sqrt(8) * np.array(motions)[:, None]
        spectrum = lines @ (mix @ forces)
        for natural in (310.0, 420.0):
            shape = rng.standard_normal(len(lines))
            drive = rng.standard_normal(8) / np.sqrt(8) @ forces
            gain = -freq ** 2 / (natural ** 2 - freq ** 2 + 0.02j * natural * freq)
            spectrum = spectrum + bending * np.outer(shape, gain * drive)
        rows = np.fft.irfft(spectrum, n=count, axis=1)
        rows = rows + noise * rng.standard_normal(rows.shape)
        return TimeHistory(start + np.arange(count) / RATE, rows, response_dof=self.dofs,
                           ordinate_dim='acceleration', ordinate_unit='m/s**2')

    def table_test(self, axis, rocking=0.0, seed=20, noise=1e-3, bending=1.0):
        """A test on a table with one drive: it moves the body along
        `axis`, 0 to 2, with 3 % of motion across it, and turns it in
        step with that, so that the rotation's acceleration 0.3 m out is
        `rocking` of the translation's. The drive bends the body in its
        two modes as well, and noise is added."""
        rng = np.random.default_rng(seed + axis)
        count = int(SECONDS * RATE)
        lines = self.lines()
        freq = np.fft.rfftfreq(count, 1 / RATE)
        drive = np.fft.rfft(rng.standard_normal(count))
        along = np.eye(3)[axis]
        across = np.cross(along, rng.standard_normal(3))
        about = rng.standard_normal(3)
        motion = np.concatenate([along + 0.03 * across / np.linalg.norm(across),
                                 rocking / 0.3 * about / np.linalg.norm(about)])
        spectrum = np.outer(lines @ motion, drive)
        for natural in (310.0, 420.0):
            shape = rng.standard_normal(len(lines))
            gain = -freq ** 2 / (natural ** 2 - freq ** 2 + 0.02j * natural * freq)
            spectrum = spectrum + bending * rng.standard_normal() * np.outer(shape, gain * drive)
        rows = np.fft.irfft(spectrum, n=count, axis=1)
        rows = rows + noise * rng.standard_normal(rows.shape)
        return TimeHistory(np.arange(count) / RATE, rows, response_dof=self.dofs,
                           ordinate_dim='acceleration', ordinate_unit='m/s**2')

    def table_tests(self, rocking=0.0, **given):
        """Three table tests, one along each axis."""
        return [self.table_test(axis, rocking, **given) for axis in range(3)]

    def geometry(self, nodes=None, frames=None):
        """The sensors as a geometry: each at its place, measured in a
        coordinate system of its own, the true one or `frames`' for it."""
        nodes = list(self.nodes if nodes is None else nodes)
        cs_matrix = np.zeros((len(nodes) + 1, 4, 3))
        cs_matrix[0, :3] = np.eye(3)
        for k, node in enumerate(nodes, start=1):
            cs_matrix[k, :3] = (frames or {}).get(node, self.frame[node])
            cs_matrix[k, 3] = self.position[node]
        return Geometry(node_id=nodes, node_xyz=[self.position[node] for node in nodes],
                        node_def_cs=np.ones(len(nodes), dtype=np.int64),
                        node_disp_cs=np.arange(2, len(nodes) + 2),
                        cs_id=np.arange(1, len(nodes) + 2), cs_name=[''] * (len(nodes) + 1),
                        cs_type=np.zeros(len(nodes) + 1, dtype=np.int64), cs_matrix=cs_matrix,
                        length_unit='m')

    def known(self, nodes):
        return {node: self.position[node] for node in nodes}

    def errors(self, recovered):
        """(worst position error of a sensor of two axes or more, worst
        distance of a one-axis sensor's line from its true place, worst
        angle between a measured axis and the truth in degrees)."""
        point, line, angle = 0.0, 0.0, 0.0
        for one in recovered.sensors:
            gap = one.position - self.position[one.node]
            for axis in one.axes:
                cosine = float(one.frame[axis] @ self.frame[one.node][axis])
                angle = max(angle, float(np.degrees(np.arccos(np.clip(cosine, -1.0, 1.0)))))
            if one.slides:
                along = one.frame[one.axes[0]]
                line = max(line, float(np.linalg.norm(gap - (gap @ along) * along)))
            else:
                point = max(point, float(np.linalg.norm(gap)))
        return point, line, angle


@pytest.fixture(scope='module')
def layout():
    return Layout()


@pytest.fixture(scope='module')
def history(layout):
    return layout.history()


@pytest.fixture(scope='module')
def recovered(history):
    return mg.recover([history], averagings=[FRAMES])


FOUR = (10, 20, 30, 40)


# ---- the sensors come back ------------------------------------------------


def test_mixed_sensors_are_placed_and_turned_under_the_default_window(layout, recovered):
    """Hann frames, bending modes above the band and noise on every channel:
    the positions come back to a millimeter in 0.6 m and the axes to a
    quarter of a degree, once four known nodes give the frame."""
    assert FRAMES.window == 'hann'
    placed = mg.place(recovered, known=layout.known(FOUR), length_unit='m')
    point, line, angle = layout.errors(placed)
    assert point < 1e-3 and line < 1e-3 and angle < 0.25
    assert [one.node for one in placed.sensors] == layout.nodes
    assert [len(one.axes) for one in placed.sensors] == [len(kind) for kind in KINDS]
    assert placed.length_unit == 'm' and not placed.image_open
    # in the known nodes' frame only once they have set it: a span or two nodes size it alone
    assert placed.framed and not recovered.framed
    assert not mg.place(recovered, span=0.5).framed
    assert not mg.place(recovered, known=layout.known(FOUR[:2])).framed
    assert recovered.misfit < 0.01 and recovered.starts == 1


def test_the_band_chosen_stops_below_the_bending(recovered):
    low, high = recovered.motion.band
    assert low < 20 and 40 < high < 310
    # the evidence it was chosen on: six motions clear in every band of it, not above it
    edges, values = recovered.motion.bands, recovered.motion.values
    inside = [mg.clear_motions(row) for row, a, b in zip(values, edges[:-1], edges[1:], strict=True)
              if low <= a and b <= high]
    assert inside and set(inside) == {mg.MOTIONS}
    above = [mg.clear_motions(row) for row, a in zip(values, edges[:-1], strict=True) if a >= high]
    assert above and above[0] < mg.MOTIONS


def test_taking_the_subspace_to_zero_frequency_takes_the_bending_out(layout):
    """A band reaching well up toward the first mode. Read whole, the bending
    in it tips the six directions away from the rigid ones; fitted against
    the frequency squared and read at zero, most of that is gone."""
    history = layout.history(noise=1e-4)
    low, high = 10.0, 100.0
    pool = mg._Pool([history], mg.motion_channels(history)[0], [FRAMES])
    rigid, _r = np.linalg.qr(layout.lines())

    def off(basis):
        return float(np.linalg.norm(basis - rigid @ (rigid.T @ basis), 2))

    notes = []
    whole = mg._amplitudes(pool.cospectrum(low, high))[1][:, :mg.MOTIONS]
    at_zero = mg._at_zero(pool, low, high, pool.lines_in(low, high), notes)
    assert not notes and off(at_zero) < off(whole) / 5
    assert mg.rigid_motion([history], (low, high), averagings=[FRAMES]).basis == pytest.approx(at_zero)
    # with too few lines for it the band is read whole, and that is said
    few = []
    plain = mg._at_zero(pool, 20.0, 28.0, 8, few)
    assert few == ['the band holds 8 lines, too few to take the bending out of it: lengthen the '
                   + 'frames or widen the band']
    np.testing.assert_allclose(np.abs(plain.T @ mg._amplitudes(pool.cospectrum(20.0, 28.0))[1][:, :6]),
                               np.eye(6), atol=1e-9)


def test_a_one_axis_sensor_is_on_its_line_and_slid_to_its_known_node(layout, recovered):
    alone = mg.place(recovered, known=layout.known(FOUR))
    one = alone.sensor(90)
    assert one.slides and one.axes == (2,)
    # not known: on its line, but not at its true place along it
    gap = one.position - layout.position[90]
    along = one.frame[2]
    assert np.linalg.norm(gap - (gap @ along) * along) < 1e-3 < np.linalg.norm(gap)
    told = mg.place(recovered, known=layout.known((*FOUR, 90))).sensor(90)
    assert np.linalg.norm(told.position - layout.position[90]) < 1e-3


def test_a_negative_dof_is_the_axis_reversed():
    layout = Layout(signs={(20, 'Y'): '-', (90, 'Z'): '-'})
    assert '20Y-' in layout.dofs and '90Z-' in layout.dofs
    placed = mg.place(mg.recover([layout.history()], averagings=[FRAMES]),
                      known=layout.known(FOUR))
    # the frame is the node's own axes: the channel reads along minus its Y
    assert layout.errors(placed)[2] < 0.25


def test_a_record_that_does_not_start_at_zero_reads_the_same(layout, history, recovered):
    late = mg.recover([layout.history(start=37.5)], averagings=[FRAMES])
    assert late.motion.band == recovered.motion.band
    np.testing.assert_allclose(np.array([one.position for one in late.sensors]),
                               np.array([one.position for one in recovered.sensors]), atol=1e-9)


def test_three_triaxial_sensors_alone_are_placed_from_random_starts():
    layout = Layout(kinds=('XYZ', 'XYZ', 'XYZ'), seed=4)
    found = mg.recover([layout.history()], averagings=[FRAMES])
    # nine channels give eighteen linear conditions, too few to start from
    assert mg._linear_start(found.motion.basis, mg._sensors_of(found.motion.channels)) is None
    assert found.misfit < mg.GOOD_MISFIT and not found.notes
    # three nodes give the frame and the size, and leave the image open: the truth is one
    # of the two
    worst = min(layout.errors(mg.place(found, known=layout.known(layout.nodes),
                                       mirror=mirror))[0] for mirror in (False, True))
    assert worst < 1e-3


# ---- what the motion cannot say -------------------------------------------


def test_the_mirror_image_reads_the_same(layout):
    """Every position through the origin with the axes kept spans the same
    six directions, so no data tells the two apart."""
    true, image = layout.lines(), layout.lines(mirrored=True)
    q, _r = np.linalg.qr(true)
    assert np.linalg.norm(image - q @ (q.T @ image)) < 1e-12
    assert not np.allclose(true, image)


def test_with_nothing_known_the_size_and_the_image_are_open(layout, recovered):
    assert recovered.length_unit is None and recovered.image_open
    held = [one for one in recovered.sensors if not one.slides]
    a, b = (recovered.sensor(node) for node in recovered.span_nodes)
    gaps = [np.linalg.norm(p.position - q.position) for p in held for q in held]
    assert np.linalg.norm(a.position - b.position) == pytest.approx(1.0) == pytest.approx(max(gaps))
    # the first sensor of three axes gives the global axes
    np.testing.assert_allclose(recovered.sensors[0].frame, np.eye(3), atol=1e-12)


def test_a_span_gives_the_size_alone(layout, recovered):
    sized = mg.place(recovered, span=0.42, length_unit='m')
    a, b = (sized.sensor(node) for node in sized.span_nodes)
    assert np.linalg.norm(a.position - b.position) == pytest.approx(0.42)
    assert sized.length_unit == 'm' and sized.image_open
    # the true span between those two, to the fit's accuracy
    true = np.linalg.norm(layout.position[a.node] - layout.position[b.node])
    exact = mg.place(recovered, span=true)
    spans = [np.linalg.norm(exact.sensor(p).position - exact.sensor(q).position)
             / np.linalg.norm(layout.position[p] - layout.position[q])
             for p, q in ((10, 20), (30, 50), (60, 70))]
    np.testing.assert_allclose(spans, 1.0, atol=5e-3)


def test_mirror_takes_the_other_image_and_keeps_the_axes(recovered):
    other = mg.place(recovered, mirror=True)
    for one, was in zip(other.sensors, recovered.sensors, strict=True):
        np.testing.assert_allclose(one.position, -was.position, atol=1e-12)
        np.testing.assert_allclose(one.frame, was.frame, atol=1e-12)


def test_known_nodes_in_a_plane_leave_the_image_open(layout, recovered):
    flat = Layout()
    for node in (10, 20, 30):
        flat.position[node] = layout.position[node]
    three = mg.place(recovered, known=layout.known((10, 20, 30)))
    assert three.image_open
    both = [layout.errors(mg.place(recovered, known=layout.known((10, 20, 30)), mirror=mirror))[0]
            for mirror in (False, True)]
    # one of the two images is the truth; the three nodes fit both
    assert min(both) < 2e-3 and max(both) > 0.1
    four = mg.place(recovered, known=layout.known(FOUR))
    assert not four.image_open
    with pytest.raises(ValueError, match='nothing for Mirror to choose'):
        mg.place(recovered, known=layout.known(FOUR), mirror=True)


def test_two_known_nodes_give_the_size_and_say_so(layout, recovered):
    two = mg.place(recovered, known=layout.known((10, 20)), length_unit='m')
    gap = np.linalg.norm(two.sensor(10).position - two.sensor(20).position)
    assert gap == pytest.approx(np.linalg.norm(layout.position[10] - layout.position[20]))
    assert any('give the size' in note for note in two.notes) and two.image_open


@pytest.mark.parametrize('given, said', [
    ({'known': {10: (0, 0, 0)}}, 'shares 1 node with'),
    ({'known': {90: (0, 0, 0), 100: (1, 0, 0), 110: (0, 1, 0)}}, 'shares 0 nodes with'),
    ({'span': 0.0}, 'a length above zero'),
    ({'span': -1.0}, 'a length above zero'),
    ({'span': 1.0, 'known': {10: (0, 0, 0), 20: (1, 0, 0)}}, 'not both'),
])
def test_what_cannot_place_the_sensors_is_refused(recovered, given, said):
    with pytest.raises(ValueError, match=said):
        mg.place(recovered, **given)


# ---- several tests together -----------------------------------------------


def test_tests_that_each_move_it_some_ways_are_pooled(layout):
    """Three tests, each with the rigid motions about one axis: none places
    the sensors, nor do two, and each refusal says how far the sixth motion
    is from standing clear; together they do."""
    along = [layout.history(motions=tuple(1.0 if k % 3 == axis else 0.0 for k in range(6)),
                            seed=7 + axis) for axis in range(3)]
    for one in along:
        with pytest.raises(ValueError, match=r'six rigid motions do not stand clear in [\d.]+ '
                                             r'to [\d.]+ Hz: the sixth strongest motion is '
                                             r'[\d.]+ times the seventh, and 5 times is needed'
                                             r'.* can be picked with this one\.$'):
            mg.rigid_motion([one], averagings=[FRAMES])
    with pytest.raises(ValueError, match=r'six rigid motions do not stand clear .* to place '
                                         r'the sensors\.$'):
        mg.rigid_motion(along[:2], averagings=[FRAMES] * 2)
    # translations alone, however many tests: the rotations are not in the data, and it is
    # read as three motions with why six were not
    slides = [layout.history(motions=(1.0, 0.8, 0.6, 0.0, 0.0, 0.0), seed=11 + k) for k in range(3)]
    three = mg.rigid_motion(slides, averagings=[FRAMES] * 3)
    assert three.count == 3 and three.basis.shape == (len(layout.dofs), 3)
    assert three.without_six.startswith('six rigid motions do not stand clear')
    pooled = mg.place(mg.recover(along, averagings=[FRAMES] * 3), known=layout.known(FOUR))
    point, line, angle = layout.errors(pooled)
    assert point < 2e-3 and line < 2e-3 and angle < 0.25


def test_histories_of_other_channels_are_refused_by_the_channel(layout, history):
    fewer = Layout(kinds=KINDS[:-1]).history()
    with pytest.raises(ValueError, match='different channels: 110Y\\+ is in one and not in '
                                         'history 2'):
        mg.rigid_motion([history, fewer], averagings=[FRAMES] * 2)


def test_a_band_higher_up_with_six_clear_values_is_not_taken():
    edges = np.geomspace(8.0, 8.0 * mg.BAND_STEP ** 9, 10)
    clear = [10.0] * 6 + [1.0, 1.0]
    three = [10.0] * 3 + [1.0] * 5
    noise = [1.0] * 8
    # the data starts to move in the second band with three motions; six higher up are
    # among the elastic modes
    with pytest.raises(ValueError, match=r'do not stand clear in 10\.1 to 12\.7 Hz: the sixth strongest motion is 1\.0 times'):
        mg._clear_band(edges, np.array([noise, three, three, three, three, three, clear, clear,
                                        noise]), 1)
    # six within an octave of where it starts to move are the rigid ones
    band = mg._clear_band(edges, np.array([noise, three, clear, clear, clear, noise, clear, noise,
                                           noise]), 1)
    assert band == pytest.approx((edges[2], edges[5]))
    with pytest.raises(ValueError, match='no motion stands clear of the noise'):
        mg._clear_band(edges, np.array([noise] * 9), 1)


def test_noise_alone_has_no_clear_motion():
    """The rule that counts motions, against white noise under the default
    framing: in no band of any of twenty records does anything stand clear."""
    tripped = 0
    for seed in range(20):
        rng = np.random.default_rng(seed)
        count = int(8 * RATE)
        dofs = [f'{node}{axis}+' for node in (1, 2, 3, 4) for axis in 'XYZ']
        noise = TimeHistory(np.arange(count) / RATE, rng.standard_normal((12, count)),
                            response_dof=dofs, ordinate_dim='acceleration',
                            ordinate_unit='m/s**2')
        with pytest.raises(ValueError, match='no motion stands clear of the noise'):
            mg.rigid_motion([noise], averagings=[noise.suggest_averaging()])
        pool = mg._Pool([noise], mg.motion_channels(noise)[0], [noise.suggest_averaging()])
        edges = mg._candidate_bands(pool)
        tripped += sum(mg.clear_motions(mg._amplitudes(pool.cospectrum(a, b))[0]) > 0
                       for a, b in itertools.pairwise(edges))
    assert tripped == 0


# ---- what is read, and what is refused ------------------------------------


def _with(history, rows=None, **changes):
    """`history` with records replaced or its fields changed."""
    ordinate = np.array(history.ordinate, dtype=float)
    for row, values in (rows or {}).items():
        ordinate[row] = values
    fields = {'response_dof': list(history.response_dof),
              'ordinate_dim': list(history.ordinate_dim),
              'ordinate_unit': list(history.ordinate_unit)}
    fields.update(changes)
    return TimeHistory(history.abscissa, ordinate, **fields)


def test_a_dead_a_constant_and_a_nan_channel_are_refused_by_name(history):
    count = len(history.abscissa)
    with pytest.raises(ValueError, match=r'^20Y\+ does not move in .* dead channel'):
        mg.rigid_motion([_with(history, {4: np.zeros(count)})], averagings=[FRAMES])
    with pytest.raises(ValueError, match=r'^20Y\+ does not move in'):
        mg.rigid_motion([_with(history, {4: np.full(count, 3.0)})], averagings=[FRAMES])
    holed = np.array(history.ordinate[7], dtype=float)
    holed[5000] = np.nan
    with pytest.raises(ValueError, match=r'^30Y\+ holds a sample that is not a number'):
        mg.rigid_motion([_with(history, {7: holed})], averagings=[FRAMES])


def test_an_empty_selection_and_too_few_channels_are_refused(history):
    with pytest.raises(ValueError, match='no time history to read'):
        mg.rigid_motion([])
    with pytest.raises(ValueError, match='^0 channels of translation; at least seven'):
        mg.rigid_motion([history], records=[[]], averagings=[FRAMES])
    with pytest.raises(ValueError, match='^6 channels of translation; at least seven'):
        mg.rigid_motion([history], records=[range(6)], averagings=[FRAMES])


def test_what_is_not_a_translation_with_a_unit_is_left_out_and_said(history):
    dofs = list(history.response_dof)
    dims = list(history.ordinate_dim)
    units = list(history.ordinate_unit)
    dofs[-1], dofs[-2] = '110RY+', 'Current'
    dims[-3], units[-3] = 'force', 'N'
    mixed = _with(history, response_dof=dofs, ordinate_dim=dims, ordinate_unit=units)
    channels, notes = mg.motion_channels(mixed)
    assert [channel.dof for channel in channels] == list(history.response_dof[:-3])
    assert notes == ['Current is left out: it names no node',
                     '110RY+ is left out: it is not a translation',
                     '1 record of other quantities left out']
    raw = TimeHistory(history.abscissa, history.ordinate, response_dof=list(history.response_dof),
                      dimension_hint='acceleration')
    channels, notes = mg.motion_channels(raw, [0])
    assert not channels and notes == ['10X+ is left out: its unit is not declared']


def test_two_quantities_and_two_channels_on_one_axis_are_refused(history):
    dims = list(history.ordinate_dim)
    units = list(history.ordinate_unit)
    dims[0], units[0] = 'velocity', 'm/s'
    with pytest.raises(ValueError, match='the channels are acceleration and velocity'):
        mg.motion_channels(_with(history, ordinate_dim=dims, ordinate_unit=units))
    dofs = list(history.response_dof)
    dofs[1] = '10X-'
    with pytest.raises(ValueError, match=r'10X\+ and 10X- are both the X axis of node 10'):
        mg.motion_channels(_with(history, response_dof=dofs))


def test_a_band_is_checked_where_it_is_given(layout, history):
    with pytest.raises(ValueError, match='from a lower frequency to a higher one, not 80 to 20'):
        mg.rigid_motion([history], (80.0, 20.0), averagings=[FRAMES])
    with pytest.raises(ValueError, match=r'no line of the spectra lies in 3\.2 to 3\.9 Hz; the '
                                         'lines are 1 Hz apart'):
        mg.rigid_motion([history], (3.2, 3.9), averagings=[FRAMES])
    # across the first mode the bending is a seventh motion as strong as the rest
    with pytest.raises(ValueError, match='six rigid motions do not stand clear in 250 to 350 Hz'):
        mg.rigid_motion([history], (250.0, 350.0), averagings=[FRAMES])
    few = mg.rigid_motion([history], (20.0, 28.0), averagings=[FRAMES])
    assert few.lines == 8 and any('8 lines, too few' in note for note in few.notes)
    assert few.band == (20.0, 28.0)


# ---- DOFs labeled wrong ---------------------------------------------------


def _relabeled(history, change):
    """`history` with the DOFs of `change` ({as it is: as it is labeled}) renamed."""
    return _with(history, response_dof=[change.get(dof, dof) for dof in history.response_dof])


def _as_the_data_has_it(layout, found):
    """The worst angle, degrees, between the way a channel is said to read
    and the way its sensor truly points, over the channels placed, once
    four known nodes give the frame."""
    found = mg.place(found, known=layout.known(FOUR))
    true = layout.lines()[:, :3]
    worst = 0.0
    for row, channel in enumerate(found.motion.channels):
        along = found.reads_along(channel)
        if along is not None:
            worst = max(worst, float(np.degrees(np.arccos(np.clip(along @ true[row], -1, 1)))))
    return worst


def _places_off(layout, found):
    """The worst position error of the sensors of two axes or more, once
    four known nodes give the frame."""
    placed = mg.place(found, known=layout.known(FOUR))
    return max(float(np.linalg.norm(one.position - layout.position[one.node]))
               for one in placed.sensors if not one.slides)


@pytest.mark.parametrize('change', [
    {'20X+': '20Y+', '20Y+': '20X+'},
    {'20Z+': '20Z-'},
    {'20X+': '20X-'},
    {'20Y+': '20Z+', '20Z+': '20Y+'},
])
def test_a_left_handed_sensor_is_named_and_the_others_are_not_moved(layout, history, change):
    """Two axes exchanged, or one reversed, at a tri-axial sensor: it is a
    left-handed set, which the fit names. The sensor is still placed, every
    channel is drawn the way it truly reads, and the other sensors are where
    they are with the labels right."""
    found = mg.recover([_relabeled(history, change)], averagings=[FRAMES])
    assert [note for note in found.notes if 'left-handed' in note] == [
        'sensor 20: its X, Y and Z are a left-handed set as labeled, so one of them is reversed '
        + 'or two are exchanged; the data cannot say which. It is placed, and the coordinate '
        + 'system it is given has its Z reversed']
    assert not any('not explained' in note for note in found.notes) and not found.left_out
    flagged = {one.node: (one.reversed, one.note) for one in found.sensors if one.note}
    assert flagged == {20: ((2,), 'left-handed as labeled')}
    assert found.misfit < 0.01 and len(found.sensors) == 11
    assert _places_off(layout, found) < 2e-3
    assert _as_the_data_has_it(layout, found) < 0.25
    # its frame is a right-handed one all the same, as a coordinate system must be: its X
    # and Y as its channels read, and its Z against the way its Z channel reads
    one = found.sensor(20)
    np.testing.assert_allclose(np.linalg.det(one.frame), 1.0)
    np.testing.assert_allclose(one.reads_along(0), one.frame[0])
    np.testing.assert_allclose(one.reads_along(1), one.frame[1])
    np.testing.assert_allclose(one.reads_along(2), -one.frame[2])
    np.testing.assert_allclose(mg.assemble(found).cs_matrix[2, :3], one.frame)


def test_two_left_handed_sensors_are_both_named(layout, history):
    found = mg.recover([_relabeled(history, {'20Z+': '20Z-', '40X+': '40Y+', '40Y+': '40X+'})],
                       averagings=[FRAMES])
    assert {one.node for one in found.sensors if one.reversed} == {20, 40}
    assert _places_off(layout, found) < 2e-3 and _as_the_data_has_it(layout, found) < 0.25


def test_two_left_handed_sensors_are_both_named_from_any_basis_of_their_motion(history):
    """Released first, sensor 40 leaves sensor 20 still wrong among the
    rest, and the fit stays poor. The next sensor released is chosen from
    that fit's poses, so it must be the linear start's: random starts there
    find layouts that fit the rest better and point at a sound sensor,
    which the basis the motion is given in decides."""
    from dataclasses import replace

    motion = mg.rigid_motion([_relabeled(history, {'20Z+': '20Z-', '40X+': '40Y+',
                                                   '40Y+': '40X+'})], averagings=[FRAMES])
    rng = np.random.default_rng(3)
    named = []
    for _ in range(8):
        turn, _r = np.linalg.qr(rng.standard_normal((mg.MOTIONS, mg.MOTIONS)))
        found = mg.fit_poses(replace(motion, basis=motion.basis @ turn))
        named.append(({one.node for one in found.sensors if one.reversed},
                      found.misfit < mg.GOOD_MISFIT))
    assert named == [({20, 40}, True)] * 8


def test_a_channel_on_the_wrong_node_is_left_out_and_said_where_it_measures(layout, history):
    """The Z channels of two tri-axial sensors exchanged: each is left out of
    the node it is labeled at and said to pass through the other sensor, and
    both sensors are placed by the two channels that are theirs."""
    found = mg.recover([_relabeled(history, {'20Z+': '40Z+', '40Z+': '20Z+'})],
                       averagings=[FRAMES])
    assert sorted(found.left_out) == ['20Z+', '40Z+']
    assert [note for note in found.notes if 'does not measure' in note] == [
        '20Z+ does not measure at node 20 with 20X+ and 20Y+: its line passes through sensor '
        + '40. It is left out',
        '40Z+ does not measure at node 40 with 40X+ and 40Y+: its line passes through sensor '
        + '20. It is left out']
    assert found.sensor(20).axes == (0, 1) and found.sensor(40).axes == (0, 1)
    assert found.sensor(20).note == '20Z+ left out' and not found.sensor(30).note
    assert sorted(channel.dof for channel in found.motion.channels
                  if found.reads_along(channel) is None) == ['20Z+', '40Z+']
    assert found.misfit < 0.01 and _places_off(layout, found) < 2e-3
    assert _as_the_data_has_it(layout, found) < 0.25


def test_a_one_axis_sensor_labeled_onto_another_node_is_left_out(layout, history):
    found = mg.recover([_relabeled(history, {'90Z+': '60Z+'})], averagings=[FRAMES])
    assert found.left_out == ('60Z+',)
    assert [note for note in found.notes if 'does not measure' in note] == [
        '60Z+ does not measure at node 60 with 60X+ and 60Y+: it measures somewhere else. It is '
        + 'left out']
    assert 90 not in [one.node for one in found.sensors] and found.sensor(60).axes == (0, 1)
    assert _places_off(layout, found) < 2e-3


@pytest.mark.parametrize('change', [
    {'20X+': '20Y+', '20Y+': '20Z+', '20Z+': '20X+'},
    {'20X+': '20X-', '20Y+': '20Y-'},
    {'60X+': '60Y+', '60Y+': '60X+'},
    {'90Z+': '90Z-'},
])
def test_labels_that_leave_a_layout_that_fits_are_not_found(layout, history, change):
    """A cyclic exchange of a sensor's axes, two reversed axes, the axes of a
    two-axis sensor exchanged, and a one-axis sensor's polarity: each is the
    same sensor turned another way, so the fit is as good and nothing is said."""
    found = mg.recover([_relabeled(history, change)], averagings=[FRAMES])
    assert found.misfit < 0.01 and not found.notes and not found.left_out
    assert not any(one.note or one.reversed for one in found.sensors)
    assert _places_off(layout, found) < 2e-3


def test_too_few_sensors_to_look_into_a_poor_fit_say_only_that_it_is_poor():
    """Three tri-axial sensors and one of two axes give just the conditions a
    linear start needs. With one sensor released there are too few, so its
    wrong label is not named: the fit is said to be poor."""
    layout = Layout(kinds=('XYZ', 'XYZ', 'XYZ', 'XY'), seed=2)
    measured = layout.history()
    assert mg.recover([measured], averagings=[FRAMES]).misfit < 0.01
    found = mg.recover([_relabeled(measured, {'20X+': '20Y+', '20Y+': '20X+'})],
                       averagings=[FRAMES])
    assert found.misfit > mg.POOR_MISFIT and not found.left_out
    assert [note for note in found.notes if 'left-handed' in note] == []
    assert any('not explained by rigid sensors' in note for note in found.notes)


# ---- the geometry ---------------------------------------------------------


def test_the_geometry_is_a_node_and_a_system_per_sensor(layout, recovered):
    placed = mg.place(recovered, known=layout.known(FOUR), length_unit='m')
    geometry = mg.assemble(placed)
    assert geometry.node_id.tolist() == layout.nodes and geometry.length_unit == 'm'
    assert geometry.cs_name == ['', *(f'Sensor {node}' for node in layout.nodes)]
    assert geometry.node_def_cs.tolist() == [1] * 11
    assert geometry.node_disp_cs.tolist() == list(range(2, 13))
    for row, one in enumerate(placed.sensors):
        np.testing.assert_allclose(geometry.node_xyz[row], one.position)
        np.testing.assert_allclose(geometry.cs_matrix[row + 1, :3], one.frame)
        np.testing.assert_allclose(geometry.cs_matrix[row + 1, 3], one.position)
        # the DOF's direction on the geometry is the axis the channel measured
        np.testing.assert_allclose(geometry.dof_direction(f'{one.node}{"XYZ"[one.axes[0]]}+'),
                                   one.frame[one.axes[0]], atol=1e-12)
    assert mg.assemble(recovered).length_unit is None


# ---- the project's verbs --------------------------------------------------


@pytest.fixture
def project(layout, history):
    out = Project()
    out.add('Run', _with(history))
    return out


def test_the_reading_changes_nothing_and_frames_a_bare_history(project, layout, history):
    journal = list(project.journal)
    # a record that does not hold a whole number of the synthesis's periods, as no
    # measurement does
    cut = 20000
    project.add('Cut', TimeHistory(history.abscissa[:cut], np.asarray(history.ordinate)[:, :cut],
                                   response_dof=list(history.response_dof),
                                   ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    journal.append(project.journal[-1])
    assert project['Cut'].averaging is None
    found = project.motion_sensors('Cut')
    assert list(project) == ['Run', 'Cut'] and project.journal == journal
    assert project['Cut'].averaging is None
    assert found.length_unit is None and len(found.sensors) == 11
    # the detector's frames, windowed: read as one whole unwindowed frame, the modes' lines
    # leak into the band and the sensors land a centimeter off
    assert found.motion.band[0] > 5
    assert layout.errors(mg.place(found, known=layout.known(FOUR)))[0] < 2e-3


def test_the_verb_adds_the_geometry_linked_and_journaled(project, layout):
    from hertzforge import Geometry

    known = Geometry(node_id=list(FOUR), node_xyz=[layout.position[node] for node in FOUR],
                     length_unit='m')
    project.add('Some Nodes', known)
    made = project.geometry_from_motion('Run', known='Some Nodes')
    assert made == 'Run Sensors' and list(project) == ['Run', 'Some Nodes', 'Run Sensors']
    geometry = project[made]
    assert geometry.length_unit == 'm' and geometry.num_nodes == 11
    np.testing.assert_allclose(geometry.node_xyz[geometry.node_index([50])[0]],
                               layout.position[50], atol=1e-3)
    assert project.object_group_of('Run') == ['Run', 'Run Sensors']
    assert project.journal[-1].startswith("project.geometry_from_motion('Run', ")
    again = project.geometry_from_motion('Run', span=0.5, name='Guess')
    assert again == 'Guess' and project['Guess'].length_unit == 'm'
    # the histories' group has its geometry: the second stands alone
    assert project.object_group_of('Guess') is None
    assert project.geometry_from_motion('Run') == 'Run Sensors (2)'
    assert project['Run Sensors (2)'].length_unit is None


def test_the_records_picked_are_the_ones_read(project):
    rows = [row for row, dof in enumerate(project['Run'].response_dof)
            if not dof.startswith(('90', '100', '110'))]
    found = project.motion_sensors('Run', records={'Run': rows})
    assert [one.node for one in found.sensors] == [10, 20, 30, 40, 50, 60, 70, 80]
    with pytest.raises(ValueError, match='records are picked under Other, which is not'):
        project.add('Other', _with(project['Run']))
        project.motion_sensors('Run', records={'Other': rows})


def test_what_is_not_a_history_or_a_geometry_is_refused(project):
    with pytest.raises(TypeError, match='Run Sensors is not a time history'):
        project.geometry_from_motion('Run')
        project.motion_sensors('Run', 'Run Sensors')
    with pytest.raises(TypeError, match='Run is not a geometry'):
        project.motion_sensors('Run', known='Run')
    with pytest.raises(ValueError, match='pick one or more time histories'):
        project.motion_sensors()


def test_it_is_offered_on_a_history_with_seven_translations_of_one_quantity(project, history):
    from hertzforge.service import projects

    assert 'geometry_from_motion' in projects.verbs_of(project, 'Run')
    few = TimeHistory(history.abscissa, history.ordinate[:6],
                      response_dof=list(history.response_dof[:6]),
                      ordinate_dim='acceleration', ordinate_unit='m/s**2')
    project.add('Few', few)
    forces = TimeHistory(history.abscissa, history.ordinate[:9],
                         response_dof=list(history.response_dof[:9]),
                         ordinate_dim='force', ordinate_unit='N')
    project.add('Forces', forces)
    assert 'geometry_from_motion' not in projects.verbs_of(project, 'Few')
    assert 'geometry_from_motion' not in projects.verbs_of(project, 'Forces')


# ---- three translations: the sensors are turned and not placed -------------


@pytest.fixture(scope='module')
def on_table(layout):
    return layout.table_tests()


@pytest.fixture(scope='module')
def turned(on_table):
    return mg.recover(on_table, averagings=[FRAMES] * 3)


def test_three_table_tests_turn_the_sensors_and_place_none(layout, turned):
    """Each test alone is one motion and the three are the translations:
    every sensor's axes come back under the default window, and no
    position does."""
    assert turned.turned_only and turned.motion.count == 3
    assert not any(one.placed for one in turned.sensors)
    assert turned.misfit < 0.002 and not turned.image_open and turned.length_unit is None
    told = mg.turn(turned, frames=layout.frame)
    assert told.agree == (len(layout.dofs), len(layout.dofs))
    assert set(told.off) == set(layout.dofs) and max(told.off.values()) < 0.25
    # turned into the geometry's frame, and with no position still nowhere in it
    assert not told.framed
    assert not told.notes and not any(one.note for one in told.sensors)
    # the frame it comes in before any geometry is known is the first three-axis sensor's
    assert np.allclose(turned.sensor(10).frame, np.eye(3), atol=1e-9)


def test_one_table_test_and_two_are_refused(layout, on_table):
    for some in (on_table[:1], on_table[:2]):
        with pytest.raises(ValueError, match='six rigid motions do not stand clear'):
            mg.recover(some, averagings=[FRAMES] * len(some))


def test_a_test_that_turns_it_as_much_as_it_moves_it_is_not_read_as_translations(layout):
    """One test along X with the two rotations it excites holds three
    motions that stand clear, and they are not the translations: the
    refusal is the six-motion one, with that said."""
    one = layout.history(motions=(1.0, 0.0, 0.0, 0.0, 2.5, 2.0))
    with pytest.raises(ValueError, match=r'^six rigid motions do not stand clear .* can be '
                                         r'picked with this one\. Three motions do stand clear, '
                                         r'but sensors that only translate leave \d+% of them '
                                         r'unexplained, so they are not the three translations$'):
        mg.recover([one], averagings=[FRAMES])


def test_rocking_in_step_with_the_table_tilts_the_axes_by_about_what_is_left_unexplained(layout):
    """A rotation each test has with its translation is read as part of
    it: 3 % across the sensors tilts the axes by a degree or two and
    leaves about 2 % unexplained; 10 % is warned of with the tilt; 30 %
    is not three translations."""
    some = mg.recover(layout.table_tests(rocking=0.03), averagings=[FRAMES] * 3)
    worst = max(mg.turn(some, frames=layout.frame).off.values())
    assert 0.01 < some.misfit < 0.03 and 0.5 < worst < 3.0 and not some.notes
    # every sensor is tilted its own way, and the frame is fitted to them all, not to the
    # sensor it was first tried from: no further turn brings the directions nearer
    told = mg.turn(some, frames=layout.frame)
    reads = np.array([told.reads_along(channel) for channel in told.motion.channels])
    truth = np.array([channel.sign * layout.frame[channel.node][channel.axis]
                      for channel in told.motion.channels])
    further = Rotation.from_matrix(mg._turn_onto(reads, truth, True)).magnitude()
    assert np.degrees(further) < 0.01
    more = mg.recover(layout.table_tests(rocking=0.10), averagings=[FRAMES] * 3)
    worst = max(mg.turn(more, frames=layout.frame).off.values())
    assert mg.POOR_MISFIT < more.misfit < 0.10 and 3.0 < worst < 9.0
    (note,) = more.notes
    assert note.startswith(f'{more.misfit:.0%} of the motion is not explained by sensors that '
                           'only translate') and 'by about 4 degrees' in note
    with pytest.raises(ValueError, match='leave 1[5-9]% of them unexplained'):
        mg.recover(layout.table_tests(rocking=0.30), averagings=[FRAMES] * 3)


def test_the_band_ends_where_the_three_motions_stop_being_the_same(layout, turned):
    """Three tests of one drive each hold three motions at every
    frequency, the bending they drive included, so counting motions does
    not end the band below the modes at 310 and 420 Hz. That the motions
    change does: with the bending the band stops well under the first
    mode, and without it the band runs on."""
    assert turned.motion.band[1] < 110.0
    stiff = mg.rigid_motion(layout.table_tests(bending=0.0), averagings=[FRAMES] * 3)
    assert stiff.count == 3 and stiff.band[1] > 200.0


def test_the_run_of_translating_bands_starts_where_the_data_moves_and_ends_at_a_change():
    edges = np.geomspace(8.0, 8.0 * mg.BAND_STEP ** 6, 7)
    three, noise = [10.0] * 3 + [1.0] * 5, [1.0] * 8
    same = np.eye(8)[:, :3]
    tilted = same.copy()
    tilted[:, 0] = np.cos(0.3) * same[:, 0] + np.sin(0.3) * np.eye(8)[:, 5]
    values = np.array([noise, three, three, three, three, three])
    band = mg._translating_band(edges, values, [same, same, same, same, tilted, same])
    assert band == pytest.approx((edges[1], edges[4]))
    # under the limit the motions are the same ones
    tilted[:, 0] = np.cos(0.05) * same[:, 0] + np.sin(0.05) * np.eye(8)[:, 5]
    band = mg._translating_band(edges, values, [same, same, same, same, tilted, same])
    assert band == pytest.approx((edges[1], edges[6]))
    # three that stand clear only higher up than the data starts to move are not taken
    one = [10.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]
    values = np.array([one, one, one, one, one, three])
    assert mg._translating_band(edges, values, [same] * 6) is None
    assert mg._translating_band(edges, np.array([noise] * 6), [same] * 6) is None


def test_a_band_that_is_given_is_read_as_three_motions_where_three_stand(layout, on_table):
    found = mg.recover(on_table, (10.0, 40.0), averagings=[FRAMES] * 3)
    assert found.turned_only and found.motion.band == (10.0, 40.0)
    assert max(mg.turn(found, frames=layout.frame).off.values()) < 0.25
    with pytest.raises(ValueError, match='six rigid motions do not stand clear in 10 to 40 Hz'):
        mg.recover(on_table[:2], (10.0, 40.0), averagings=[FRAMES] * 2)


def test_a_left_handed_sensor_on_the_table_is_named_and_drawn_as_it_reads(layout, on_table):
    change = {'20X+': '20Y+', '20Y+': '20X+'}
    found = mg.recover([_relabeled(one, change) for one in on_table], averagings=[FRAMES] * 3)
    odd = found.sensor(20)
    assert odd.note == 'left-handed as labeled' and odd.reversed == (2,)
    assert [one.node for one in found.sensors if one.note] == [20]
    (note,) = found.notes
    assert note.startswith('sensor 20: its X, Y and Z are a left-handed set as labeled')
    assert 'As many sensors' not in note
    told = mg.turn(found, frames=layout.frame)
    # its X and Y read along each other's true axes, and its Z as it was
    assert told.off['20Z+'] < 0.5
    assert '20X+ reads along 20Y+ of the geometry' in told.notes
    assert '20Y+ reads along 20X+ of the geometry' in told.notes
    assert told.agree == (len(layout.dofs) - 2, len(layout.dofs))


def test_two_sensors_of_opposite_hands_say_the_data_cannot_choose():
    pair = Layout(kinds=('XYZ', 'XYZ', 'XY', 'YZ', 'XZ', 'Z'), seed=4)
    tests = [_relabeled(one, {'20X+': '20Y+', '20Y+': '20X+'}) for one in pair.table_tests()]
    found = mg.recover(tests, averagings=[FRAMES] * 3)
    (note,) = found.notes
    assert note.startswith('sensor 20: its X, Y and Z are a left-handed set')
    assert note.endswith('As many sensors are of each hand, and the first is taken as '
                         'right-handed')


def test_a_channel_that_is_not_square_to_its_node_is_left_out_and_the_rest_fit(layout, on_table):
    """A one-axis sensor's channel labeled as the Z of a three-axis one is
    not at right angles to its X and Y. Left in, it spoils every sensor's
    fit; it is found by releasing the worst sensor, left out and said,
    and the others come back as if nothing were wrong."""
    change = {'40Z+': '90Z+', '90Z+': '40Z+'}
    found = mg.recover([_relabeled(one, change) for one in on_table], averagings=[FRAMES] * 3)
    assert found.left_out == ('40Z+',) and found.misfit < 0.005
    odd = found.sensor(40)
    assert odd.axes == (0, 1) and odd.note == '40Z+ left out'
    off = np.degrees(np.arcsin(abs(layout.frame[90][2] @ layout.frame[40][0])))
    off = max(off, np.degrees(np.arcsin(abs(layout.frame[90][2] @ layout.frame[40][1]))))
    (note,) = found.notes
    assert note == (f'40Z+ does not read at right angles to 40X+ and 40Y+: it is {off:.0f} '
                    'degrees off square. It is left out')
    assert found.reads_along(next(c for c in found.motion.channels if c.dof == '40Z+')) is None
    told = mg.turn(found, frames=layout.frame)
    # the channel left out is not compared; the one that took its place at node 90 is off
    assert '40Z+' not in told.off and told.agree == (len(layout.dofs) - 2, len(layout.dofs) - 1)
    assert max(angle for dof, angle in told.off.items() if dof != '90Z+') < 0.25
    # two channels of a two-axis sensor that are not square are not one sensor
    pair = {'70Y+': '100X+', '100X+': '70Y+'}
    found = mg.recover([_relabeled(one, pair) for one in on_table], averagings=[FRAMES] * 3)
    assert set(found.left_out) == {'70Y+', '70Z+'} and found.misfit < 0.005
    assert 70 not in [one.node for one in found.sensors]
    (note,) = found.notes
    assert note.startswith('sensor 70: 70Z+ and 70Y+ do not read at right angles to each other, ')
    assert note.endswith('degrees off square, so they are not one sensor. It is left out')


def test_a_known_geometry_shows_the_mistakes_that_leave_a_sensor_that_fits(layout, on_table):
    """A cyclic exchange of a sensor's axes, the polarity of a one-axis
    sensor and a sensor mounted turned all leave sensors that fit, so
    nothing is said until a geometry gives the directions they should
    read along."""
    change = {'30X+': '30Y+', '30Y+': '30Z+', '30Z+': '30X+', '90Z+': '90Z-'}
    found = mg.recover([_relabeled(one, change) for one in on_table], averagings=[FRAMES] * 3)
    assert not found.notes and found.misfit < 0.002
    spun = Rotation.from_rotvec(np.radians(40.0) * layout.frame[50][2]).as_matrix()
    frames = {**layout.frame, 50: layout.frame[50] @ spun.T}
    told = mg.turn(found, frames=frames)
    assert told.agree == (len(layout.dofs) - 6, len(layout.dofs))
    assert set(told.notes) == {
        '30X+ reads along 30Z+ of the geometry', '30Y+ reads along 30X+ of the geometry',
        '30Z+ reads along 30Y+ of the geometry',
        '90Z- reads opposite to its direction in the geometry: its polarity is reversed',
        '50X+ reads 40 degrees from its direction in the geometry',
        '50Y+ reads 40 degrees from its direction in the geometry'}
    assert told.sensor(90).note == 'Z reversed'
    assert told.sensor(30).note == 'X reads along Z+; Y reads along X+; Z reads along Y+'
    assert told.sensor(50).note == 'X 40 degrees off; Y 40 degrees off'
    assert told.off['90Z-'] > 179.0 and told.off['50Z+'] < 0.5
    assert max(angle for dof, angle in told.off.items() if dof[:2] in ('10', '20', '40')) < 0.25


def test_the_frame_is_the_one_most_channels_agree_under(layout, turned):
    """Three sensors whose coordinate systems in the geometry are wrong do
    not move the frame the others set; a geometry that has every node in
    the global system agrees with few channels under any turn, and that
    is said."""
    rng = np.random.default_rng(3)
    wrong = {node: Rotation.random(random_state=rng).as_matrix() for node in (10, 30, 60)}
    told = mg.turn(turned, frames={**layout.frame, **wrong})
    right = [angle for dof, angle in told.off.items() if int(dof[:-2]) not in wrong]
    assert max(right) < 0.25 and told.agree[0] >= len(right)
    bare = mg.turn(turned, frames={node: np.eye(3) for node in layout.nodes})
    agreeing, compared = bare.agree
    assert compared == len(layout.dofs) and agreeing < compared / 2
    assert any(note.startswith(f'only {agreeing} of {compared} channels read within 15 degrees')
               for note in bare.notes)


def test_positions_come_from_the_known_nodes_and_the_rest_have_none(layout, turned):
    some = (10, 20, 60, 90)
    told = mg.turn(turned, frames={node: layout.frame[node] for node in some},
                   positions=layout.known(some), length_unit='m')
    assert told.length_unit == 'm' and told.framed and not turned.framed
    assert [one.node for one in told.sensors if one.placed] == list(some)
    assert all(np.array_equal(told.sensor(node).position, layout.position[node]) for node in some)
    assert told.notes[-1] == ('30, 40, 50, 70, 80, 100 and others: not in the known geometry, so '
                              'turned and left without a place')
    # compared on the known nodes alone, and the others are in its frame all the same
    assert told.agree == (9, 9)
    for one in told.sensors:
        for axis in one.axes:
            assert np.degrees(np.arccos(one.frame[axis] @ layout.frame[one.node][axis])) < 0.25
    made = mg.assemble(told)
    assert list(made.node_id) == list(some) and made.length_unit == 'm'
    assert np.allclose(made.dof_direction('60Y+'), told.sensor(60).frame[1])


def test_a_one_axis_sensor_keeps_the_other_axes_the_geometry_gives_it(layout, turned):
    told = mg.turn(turned, frames=layout.frame)
    for node in (90, 100, 110):
        assert np.allclose(told.sensor(node).frame, layout.frame[node], atol=5e-3)
    # with no geometry they are any two that complete the frame
    assert not np.allclose(turned.sensor(90).frame @ turned.sensor(10).frame.T
                           @ layout.frame[10], layout.frame[90], atol=5e-3)


def test_without_a_three_axis_sensor_the_image_is_open_until_a_geometry_settles_it():
    flat = Layout(kinds=('XY', 'YZ', 'XZ', 'XY', 'YZ', 'Z', 'X', 'Y'), seed=5)
    found = mg.recover(flat.table_tests(), averagings=[FRAMES] * 3)
    assert found.turned_only and found.image_open
    told = mg.turn(found, frames=flat.frame)
    assert not told.image_open and max(told.off.values()) < 0.25
    # directions in one plane fit both images, so they do not choose
    level = {node: np.eye(3) for node in flat.nodes}
    planar = mg.turn(found, frames={node: level[node] for node in (10, 40)})
    assert planar.image_open


def test_turned_sensors_are_not_sized_moved_or_made_without_positions(layout, turned, recovered):
    with pytest.raises(ValueError, match='turned by three translations and not placed'):
        mg.place(turned, span=1.0)
    with pytest.raises(ValueError, match='pick a geometry that holds the nodes'):
        mg.assemble(turned)
    with pytest.raises(ValueError, match='gives the direction of 1 channel that are not '
                                         'parallel; two are needed'):
        mg.turn(turned, frames={90: layout.frame[90]})
    with pytest.raises(ValueError, match='gives the direction of 0 channels'):
        mg.turn(turned, frames={})
    with pytest.raises(ValueError, match='were placed by six rigid motions'):
        mg.turn(recovered, frames=layout.frame)


def test_the_project_turns_table_tests_against_a_geometry_that_holds_the_nodes(layout, on_table):
    project = Project()
    names = [project.add(f'Table {"XYZ"[k]}', one) for k, one in enumerate(on_table)]
    bare = project.motion_sensors(*names)
    assert bare.turned_only and not any(one.placed for one in bare.sensors) and not bare.off
    some = (10, 20, 30, 60, 90)
    project.add('Layout', layout.geometry(some))
    told = project.motion_sensors(*names, known='Layout')
    assert told.agree == (12, 12) and max(told.off.values()) < 0.25
    assert [one.node for one in told.sensors if one.placed] == list(some)
    before = list(project)
    with pytest.raises(ValueError, match='turned and not placed: there is no layout for a span '
                                         'to size'):
        project.motion_sensors(*names, span=0.5)
    with pytest.raises(ValueError, match='there is no layout for Mirror to reflect'):
        project.motion_sensors(*names, known='Layout', mirror=True)
    with pytest.raises(ValueError, match='pick a geometry that holds the nodes'):
        project.geometry_from_motion(*names)
    with pytest.raises(TypeError, match='Table X is not a geometry'):
        project.motion_sensors(*names, known='Table X')
    assert list(project) == before
    made = project.geometry_from_motion(*names, known='Layout')
    assert made == 'Table X Sensors' and list(project[made].node_id) == list(some)
    assert np.allclose(project[made].node_xyz, [layout.position[node] for node in some])
    assert np.degrees(np.arccos(project[made].dof_direction('30Z+') @ layout.frame[30][2])) < 0.25


def test_a_node_in_a_cylindrical_system_is_placed_and_not_compared(layout, on_table):
    project = Project()
    names = [project.add(f'Table {"XYZ"[k]}', one) for k, one in enumerate(on_table)]
    held = layout.geometry((10, 20, 30))
    held.cs_type = np.array([0, 0, 1, 0])
    project.add('Layout', held)
    told = project.motion_sensors(*names, known='Layout')
    assert told.agree == (6, 6) and not any(dof.startswith('20') for dof in told.off)
    assert told.sensor(20).placed
