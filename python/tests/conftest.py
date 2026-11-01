"""Shared fixtures: the contract's files, schema validators, and the
datasets opened once per test session."""

from __future__ import annotations

import json
from pathlib import Path

import jsonschema
import pytest

import sds_analysis  # noqa: F401  (headless before visualdynamics loads)
from sds_analysis import datasets

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


def _open(dataset_id):
    return datasets.entry(dataset_id).open()


@pytest.fixture(scope='session')
def random_run():
    # a missing file is an error naming the fetch command, never a skip
    return _open('plate-random')


@pytest.fixture(scope='session')
def drone():
    return _open('drone-fem')


@pytest.fixture(scope='session')
def plate_fem():
    return _open('plate-fem')
