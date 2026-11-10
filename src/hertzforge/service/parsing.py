"""What is typed into a list: the one id parser and the one number parser. Commas and spaces
both separate, and a run reads as the Element Groups table prints it (`1-16 33-48`), so a space
never glues two values into one."""

from __future__ import annotations

import math
import re

from .protocol import refused


def parse_id_runs(text: str, what: str = 'an id') -> list[int]:
    """'1-3, 7 9-10' as [1, 2, 3, 7, 9, 10], in the order named; a run either way round."""
    found: list[int] = []
    for part in str(text).replace(',', ' ').split():
        try:
            if '-' in part[1:]:
                head, _, tail = part[1:].partition('-')
                low, high = sorted((int(part[0] + head), int(tail)))
                found.extend(range(low, high + 1))
            else:
                found.append(int(part))
        except ValueError:
            raise refused(f'{part!r} is not {what} or a run such as 3-6') from None
    return found


def parse_picks(text: str, count: int, what: str) -> list[int]:
    """The 0-based items '1-3, 7' names, numbered from 1 as the shell shows them, each once, in
    the order typed."""
    picked = list(dict.fromkeys(parse_id_runs(text, f'a {what} number')))
    for number in picked:
        if not 1 <= number <= count:
            raise refused(f'{what}s run from 1 to {count}; {number} is outside them')
    return [number - 1 for number in picked]


def parse_optional_picks(text: str, count: int, what: str) -> list[int] | None:
    """`parse_picks`, with None for a blank `text`, which each caller reads as its own default."""
    return parse_picks(text, count, what) if text.strip() else None


def parse_numbers(text: str, what: str = 'a list of numbers') -> list[float]:
    """The numbers typed, separated by commas, spaces or semicolons; each one finite."""
    try:
        values = [float(part) for part in re.split(r'[,;\s]+', str(text).strip()) if part]
    except ValueError:
        raise refused(f'{str(text).strip()!r} is not {what}') from None
    if not all(math.isfinite(value) for value in values):
        raise refused(f'{str(text).strip()!r} is not {what}')
    return values
