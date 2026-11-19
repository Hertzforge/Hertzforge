"""A stage that cannot count itself: announced as BUSY, shown as a
moving bar, and counted again on the next ordinary report."""

from __future__ import annotations

from hertzforge.core.progress import BUSY, Ticker


def test_busy_is_told_at_once_and_the_next_tick_counts_again():
    told = []
    ticker = Ticker(lambda done, total: told.append((done, total)),
                    interval=0.0)
    ticker.add(4)
    ticker.tick()
    ticker.busy()
    ticker.tick()
    assert told == [(0, 4), (1, 4), (1, BUSY), (2, 4)]
    Ticker(None).busy()     # no callback, nothing to tell


def test_a_share_spreads_its_own_count_over_fixed_units():
    """A stage's own count fills its share and no more, whatever the
    count: 190,000 elements' ticks against a few hundred solves' left
    the solves a sliver and the bar read full for an hour."""
    told = []
    ticker = Ticker(lambda done, total: told.append((done, total)),
                    interval=0.0)
    first, second = ticker.share(200), ticker.share(800)
    assert ticker.total == 1000, 'both shares are in the total at once'
    first.add(190_000)
    first.tick(95_000)
    assert ticker.done == 100
    first.tick(95_000)
    assert ticker.done == 200, 'a finished stage fills its share exactly'
    second.add(4)
    second.tick()
    assert ticker.done == 400
    second.extend_to(6)      # the estimate grew: 1 of 7 now
    assert ticker.done == 400, 'never ticked back'
    second.tick(6)
    assert ticker.done == ticker.total == 1000
    assert [d for d, _t in told] == sorted(d for d, _t in told)


def test_a_count_after_busy_brings_the_bar_back():
    told = []
    ticker = Ticker(lambda done, total: told.append((done, total)),
                    interval=0.0)
    share = ticker.share(10)
    share.add(1000)
    share.busy()
    share.tick()             # less than a unit: nothing owed, still told
    assert told[-2:] == [(0, BUSY), (0, 10)]


class _Clock:
    """`time.monotonic` where the test says it is."""

    def __init__(self, now):
        self.now = now

    def __call__(self):
        return self.now


def test_a_report_waits_for_the_interval_unless_the_work_is_complete(monkeypatch):
    clock = _Clock(100.0)
    monkeypatch.setattr('hertzforge.core.progress.time.monotonic', clock)
    told = []
    ticker = Ticker(lambda done, total: told.append((done, total)), interval=1.0)
    ticker.add(5)
    assert told == [(0, 5)], 'the first report goes at once'
    clock.now = 100.5
    ticker.tick()
    assert told == [(0, 5)], 'inside the interval'
    clock.now = 101.0
    ticker.tick()
    assert told[-1] == (2, 5), 'exactly one interval on'
    clock.now = 101.1
    ticker.tick(3)
    assert told[-1] == (5, 5), 'complete, inside the interval all the same'
    assert len(told) == 3


def test_extend_to_raises_the_total_past_what_is_done_and_never_lowers_it():
    told = []
    ticker = Ticker(lambda done, total: told.append((done, total)), interval=0.0)
    ticker.add(10)
    ticker.tick(3)
    told.clear()
    ticker.extend_to(7)
    assert ticker.total == 10 and told == [], 'exactly what is left: nothing to raise'
    ticker.extend_to(2)
    assert ticker.total == 10 and told == [], 'never lowered'
    ticker.extend_to(8)
    assert ticker.total == 11 and told == [(3, 11)]


def test_a_shares_extend_to_grows_its_own_total_and_only_when_it_must():
    ticker = Ticker(None)
    share = ticker.share(70)
    share.add(7)
    share.tick()
    assert ticker.done == 10, 'one of seven'
    share.extend_to(6)
    assert share.total == 7, 'exactly what is left'
    share.extend_to(2)
    assert share.total == 7, 'never shrinks'
    share.extend_to(13)
    assert share.total == 14 and ticker.done == 10, 'never ticked back'
    share.tick(6)
    assert ticker.done == 35, 'seven of fourteen'


def test_a_share_with_no_count_yet_owes_nothing_and_past_its_count_is_full():
    ticker = Ticker(None)
    share = ticker.share(10)
    share.tick(5)
    assert ticker.done == 0, 'no count to be a fraction of'
    share.add(5)
    assert ticker.done == 10
    share.tick(100)
    assert ticker.done == 10, 'a share never passes its units'


def test_a_share_moves_its_parent_in_whole_units():
    ticker = Ticker(None)
    share = ticker.share(10)
    share.add(3)
    share.tick()
    assert ticker.done == 3 and isinstance(ticker.done, int), 'a third of ten, rounded down'
    alone = Ticker(None).share(4)
    alone.add(1)
    alone.tick()
    assert alone.parent.done == 4, 'a count of one fills its share'
