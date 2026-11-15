"""`schema/make_golden.py` writes the golden frames every codec is tested against. Run into a
scratch folder, it writes every frame exactly as committed in `schema/golden/`, so regenerating
the frames never changes one by accident."""

from __future__ import annotations

import importlib.util
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
GOLDEN = ROOT / 'schema' / 'golden'


def _script():
    spec = importlib.util.spec_from_file_location('make_golden', ROOT / 'schema' / 'make_golden.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_the_script_writes_every_golden_frame_as_committed(tmp_path, monkeypatch):
    script = _script()
    monkeypatch.setattr(script, 'HERE', tmp_path)
    script.main()
    written = sorted(p.name for p in tmp_path.iterdir() if p.is_file())
    assert written, 'the script wrote nothing'
    missing = [name for name in written if not (GOLDEN / name).is_file()]
    differ = [name for name in written
              if (GOLDEN / name).is_file() and (GOLDEN / name).read_bytes() != (tmp_path / name).read_bytes()]
    assert not missing, f'written but not committed: {missing}'
    assert not differ, f'written differently from the committed frame: {differ}'
