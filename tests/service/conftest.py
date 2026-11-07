"""Shared fixtures: the contract's files and schema validators."""

from __future__ import annotations

import json
from pathlib import Path

import jsonschema
import pytest

from hertzforge.service import frames

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_DIR = ROOT / 'schema'
GOLDEN = SCHEMA_DIR / 'golden'


@pytest.fixture(scope='session')
def schema() -> dict:
    return json.loads((SCHEMA_DIR / 'protocol.schema.json').read_text(encoding='utf-8'))


@pytest.fixture(scope='session')
def validate(schema):
    """validate(instance, name): check against `#/$defs/<name>`, or against
    the whole protocol (any message) when `name` is None."""
    def make(name):
        if name is None:
            return jsonschema.Draft202012Validator(schema)
        return jsonschema.Draft202012Validator(
            {'$schema': schema['$schema'], '$defs': schema['$defs'],
             '$ref': f'#/$defs/{name}'})

    cache = {}

    def check(instance, name=None):
        if name not in cache:
            cache[name] = make(name)
        cache[name].validate(instance)

    return check


def wire_views(views, validate):
    """The views as they travel, references in place, checked."""
    wire, arrays = frames.to_wire({'views': views})
    validate(wire['views'], 'views')
    return wire, arrays
