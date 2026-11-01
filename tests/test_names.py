"""Objects are named for what they are, not for the file they arrived in.

An importer's keys are for code — `time_data`, `Modal_frf`, `ChannelTable` —
and three ways of spelling the same idea. The tree shows one way, written for
reading.
"""

from __future__ import annotations

import pytest

from hertzforge.names import display_name


@pytest.mark.parametrize('key,expected', [
    ('time_data', 'Time Data'),
    ('channel_table', 'Channel Table'),
    ('Modal_coherence', 'Modal Coherence'),
    ('Random_specification', 'Random Specification'),
    # class names are the same words with the spaces left out
    ('ShapeSet', 'Shape Set'),
    ('TimeHistory', 'Time History'),
    ('MultipleCoherence', 'Multiple Coherence'),
    ('Geometry', 'Geometry'),
    # already a name: never reformatted underneath a user's own rename
    ('Modal FRF', 'Modal FRF'),
])
def test_a_key_reads_as_a_name(key, expected):
    assert display_name(key) == expected


@pytest.mark.parametrize('key,expected', [
    ('Frf', 'FRF'),
    ('Modal_frf', 'Modal FRF'),
    ('Random_drive_cpsd', 'Random Drive CPSD'),
    ('Random_response_cpsd', 'Random Response CPSD'),
    ('Psd', 'PSD'),
])
def test_the_acronyms_this_field_uses_stay_acronyms(key, expected):
    """'Frf' is not a word and reads as one. Capitalizing the first letter of
    each word is the rule; these are the exceptions."""
    assert display_name(key) == expected
