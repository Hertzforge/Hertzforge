"""Hertzforge carries no Qt, no VTK and no plotting stack: the shell draws.

Visual Dynamics' code came with its Qt interface, VTK scenes and pyqtgraph
plots left behind, and its drawing decisions moved into
`hertzforge.drawing` without the code that draws. These tests hold that
line, for an import written at the top of a module and for one written
inside a function, which only fails when the function is called.
"""

import ast
import pathlib
import subprocess
import sys

import hertzforge

PACKAGE = pathlib.Path(hertzforge.__file__).parent

#: what the package must never import
INTERFACE = ('PySide6', 'shiboken6', 'qtpy', 'pyqtgraph', 'pyvista', 'pyvistaqt', 'vtk',
             'vtkmodules', 'matplotlib')


def imports_of(path):
    """(line, top-level module) for every absolute import in a file."""
    found = []
    for node in ast.walk(ast.parse(path.read_text(encoding='utf-8'))):
        if isinstance(node, ast.Import):
            found += [(node.lineno, alias.name.split('.')[0]) for alias in node.names]
        elif isinstance(node, ast.ImportFrom) and node.module and not node.level:
            found.append((node.lineno, node.module.split('.')[0]))
    return found


def test_no_module_imports_the_interface():
    offending = [f'{path.relative_to(PACKAGE)}:{line} imports {module}'
                 for path in sorted(PACKAGE.rglob('*.py'))
                 for line, module in imports_of(path) if module in INTERFACE]
    assert offending == []


def test_the_check_sees_an_import_inside_a_function(tmp_path):
    planted = tmp_path / 'planted.py'
    planted.write_text('def draw():\n    import pyqtgraph as pg\n    return pg\n',
                       encoding='utf-8')
    assert (2, 'pyqtgraph') in imports_of(planted)


def test_importing_every_module_loads_none_of_it():
    """In a fresh interpreter, since another test may have imported one."""
    code = (
        'import importlib, pkgutil, sys, hertzforge\n'
        'for m in pkgutil.walk_packages(hertzforge.__path__, "hertzforge."):\n'
        '    importlib.import_module(m.name)\n'
        f'print(sorted({{k.split(".")[0] for k in sys.modules}} & set({INTERFACE!r})))\n')
    done = subprocess.run([sys.executable, '-c', code], capture_output=True, text=True,
                          timeout=300, check=False)
    assert done.returncode == 0, done.stderr[-2000:]
    assert done.stdout.strip() == '[]'
