"""Every public attribute says what it is.

Explicit rather than implicit, which is the rule this project already
follows for links and project structure — and the reason the API
reference can say `node_id: IdArray` instead of `ndarray`, and an editor
can complete against it.

An annotation is a *claim*, and a claim is only worth what enforces it:
`core/validate.py` is the run-time half, and this is the one that stops
the claims from quietly going missing as the code grows. It walks the
package's own AST, so it needs nothing imported and costs milliseconds.

What counts as declared: an annotated assignment anywhere in the class
(`self.x: int = 1`, or a bare `self.x: int` before a branch), or a
class-level annotation. A name that is a **property** is declared by its
getter — `MainWindow.project_type` is a property over the Project, so
`self.project_type = ...` inside the class is a setter call and not a
declaration at all.
"""

from __future__ import annotations

import ast
import pathlib

from conftest import public_definitions

ROOT = pathlib.Path(__file__).resolve().parent.parent

PACKAGE = ROOT / 'src' / 'hertzforge'


def declared(cls: ast.ClassDef) -> set[str]:
    """Names this class declares: annotations, and properties."""
    names = set()
    for node in ast.walk(cls):
        if isinstance(node, ast.AnnAssign):
            target = node.target
            if isinstance(target, ast.Name):
                names.add(target.id)
            elif (isinstance(target, ast.Attribute)
                  and isinstance(target.value, ast.Name)
                  and target.value.id == 'self'):
                names.add(target.attr)
    for node in cls.body:
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for decorator in node.decorator_list:
            root = decorator
            while isinstance(root, ast.Attribute):
                root = root.value
            if isinstance(root, ast.Name) and root.id in (
                    'property', 'cached_property', node.name):
                names.add(node.name)
    return names


def undeclared(path: pathlib.Path) -> list[tuple[str, str, int]]:
    """(class, attribute, line) for public attributes with no annotation."""
    tree = ast.parse(path.read_text(encoding='utf-8'))
    found, seen = [], set()
    for cls in ast.walk(tree):
        if not isinstance(cls, ast.ClassDef):
            continue
        known = declared(cls)
        for node in ast.walk(cls):
            if not isinstance(node, ast.Assign):
                continue
            for target in node.targets:
                if not (isinstance(target, ast.Attribute)
                        and isinstance(target.value, ast.Name)
                        and target.value.id == 'self'):
                    continue
                key = (cls.name, target.attr)
                if (target.attr.startswith('_') or target.attr in known
                        or key in seen):
                    continue
                seen.add(key)
                found.append((cls.name, target.attr, node.lineno))
    return found


def test_every_public_attribute_is_declared():
    """277 of them were not, when this was written. The ones that hurt
    were in the data model, where `node_id: ndarray` told a reader
    nothing about whether a string would do."""
    gaps = []
    for path in sorted(PACKAGE.rglob('*.py')):
        for cls, attr, line in undeclared(path):
            gaps.append(f'{path.relative_to(ROOT)}:{line} {cls}.{attr}')
    assert not gaps, (
        'these public attributes carry no type — annotate them at their '
        'first assignment:\n  ' + '\n  '.join(gaps[:20]))


def test_the_check_can_actually_find_one(tmp_path):
    """A checker that passes because it looks in the wrong place is
    worse than none."""
    module = tmp_path / 'sample.py'
    module.write_text(
        'class Thing:\n'
        '    def __init__(self):\n'
        '        self.declared: int = 1\n'
        '        self.bare = 2\n'
        '        self._private = 3\n'
        '    @property\n'
        '    def owned(self):\n'
        '        return 4\n'
        '    @owned.setter\n'
        '    def owned(self, value):\n'
        '        self.owned = value\n')
    found = {attr for _cls, attr, _line in undeclared(module)}
    assert found == {'bare'}, (
        'the annotated one, the private one and the property must all be '
        f'left alone; found {found}')


def _is_static(fn: ast.FunctionDef) -> bool:
    return any(isinstance(d, ast.Name) and d.id == 'staticmethod'
               for d in fn.decorator_list)


def unannotated(path: pathlib.Path, package: pathlib.Path = PACKAGE) -> list[str]:
    """'name: what is missing' for each public function or method whose
    signature leaves a parameter or the return untyped. `self` and `cls`
    are typed by where they sit, and `__init__` returns None by
    definition, so neither is asked for."""
    found = []
    for name, node, owner in public_definitions(path, package):
        if isinstance(node, ast.ClassDef):
            continue
        args = node.args
        params = args.posonlyargs + args.args + args.kwonlyargs
        if owner is not None and params and not _is_static(node):
            params = params[1:]
        missing = [a.arg for a in params if a.annotation is None]
        if args.vararg is not None and args.vararg.annotation is None:
            missing.append('*' + args.vararg.arg)
        if args.kwarg is not None and args.kwarg.annotation is None:
            missing.append('**' + args.kwarg.arg)
        if node.returns is None and node.name != '__init__':
            missing.append('the return')
        if missing:
            found.append(f'{name}:{node.lineno} {", ".join(missing)}')
    return found


def test_every_public_signature_is_typed():
    """The attributes say what they are; so does every argument a caller
    passes and every value a caller gets back. The docstring's Parameters
    table says the same thing in words, for a reader; the annotation says
    it to an editor and a type checker, which do not read prose."""
    gaps = [f'{path.relative_to(ROOT)}:{gap}'
            for path in sorted(PACKAGE.rglob('*.py'))
            for gap in unannotated(path)]
    assert not gaps, (
        f'{len(gaps)} public signatures leave something untyped:\n  '
        + '\n  '.join(gaps[:30]))


def test_the_signature_check_can_actually_find_one(tmp_path):
    """Each gap it claims to see, planted, and each thing it must leave
    alone beside it."""
    package = tmp_path / 'pkg'
    package.mkdir()
    module = package / 'sample.py'
    module.write_text(
        'def typed(a: int, *rest: int, **more: str) -> int: ...\n'
        'def bare_param(a, b: int) -> int: ...\n'
        'def bare_return(a: int): ...\n'
        'def bare_star(*rest, **more) -> None: ...\n'
        'def _private(a): ...\n'
        'class Thing:\n'
        '    def __init__(self, size): ...\n'
        '    def method(self, x: int) -> int: ...\n'
        '    @classmethod\n'
        '    def make(cls) -> "Thing": ...\n'
        '    @staticmethod\n'
        '    def helper(value) -> int: ...\n'
        '    def _hidden(self, y): ...\n'
        'class _Private:\n'
        '    def method(self, z): ...\n')
    (package / '_inner.py').write_text('def exposed(a): ...\n')
    found = [gap.split(':')[0] + ' ' + gap.split(' ', 1)[1]
             for path in sorted(package.glob('*.py'))
             for gap in unannotated(path, package)]
    assert found == ['bare_param a', 'bare_return the return',
                     'bare_star *rest, **more', 'Thing.__init__ size',
                     'Thing.helper value'], found
