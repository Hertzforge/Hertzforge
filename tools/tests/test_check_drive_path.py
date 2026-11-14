"""Each of the drive path's checks, on the tree and on planted cases: one that
must trip the check and one beside it that must not. A check that cannot
fail proves nothing."""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import textwrap
from collections.abc import Iterable

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import check_drive_path as cdp

ROOT_MANIFEST = '[workspace]\nmembers = ["crates/*"]\n'

FILLED = textwrap.dedent('''\
    ## What and why

    A change.

    ## Drive path and hardware

    <!-- the template's guidance -->

    - What can now reach the drive: the new ramp's last block.
    - How it fails safe: a NaN level is refused; `a_nan_level_is_refused` shows it.
    - What ran on the simulator: the stop ramp suite, 40 runs.
    - What ran on hardware: Not on hardware.

    ## Names and licenses

    Pass.
    ''')

TEMPLATE = textwrap.dedent('''\
    ## Drive path and hardware

    <!-- If not, keep the first line below. -->

    No drive-path code touched.

    - What can now reach the drive:
    - How it fails safe:
    - What ran on the simulator:
    - What ran on hardware:

    ## Names and licenses
    ''')


FORMAT = textwrap.dedent('''\
    # The Format

    ## `geometry/`

    Points.

    ## `run/`

    A run.

    ### Its Streams

    Samples.

    ## `shapes/`

    Modes.
    ''')

RUN_FILLED = textwrap.dedent('''\
    ## Run data

    - Adds to the setup: none
    - Adds to the table: none
    - Adds to the results: `peak_level`, the highest level reached
    - Adds to the timeline: none
    - Recorded in: not recorded yet; the run folder has no results file
    - Seeds from: none
    - Flows to: none
    - Bridge test: row not possible yet

    ## Test results
    ''')

RUN_TEMPLATE = textwrap.dedent('''\
    ## Run data

    <!-- If not, keep the first line below. -->

    No run data touched.

    - Adds to the setup:
    - Adds to the table:
    - Adds to the results:
    - Adds to the timeline:
    - Recorded in:
    - Seeds from:
    - Flows to:
    - Bridge test:

    ## Test results
    ''')


def _write(root: pathlib.Path, files: dict[str, str]) -> None:
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(text), encoding='utf-8')


def _crate(name: str, lib: str = '', extra_manifest: str = '',
           more: dict[str, str] | None = None) -> dict[str, str]:
    """The files of a crate under crates/: its manifest, src/lib.rs and `more`."""
    files = {
        f'crates/{name}/Cargo.toml':
            f'[package]\nname = "{name}"\nversion = "0.1.0"\n{extra_manifest}',
        f'crates/{name}/src/lib.rs': lib,
    }
    files.update({f'crates/{name}/{k}': v for k, v in (more or {}).items()})
    return files


def _check(root: pathlib.Path, lines: str) -> list[str]:
    entries, problems = cdp.parse_safety_tests(textwrap.dedent(lines))
    assert problems == []
    return cdp.check_safety_tests(entries, root)


# ------------------------------------------------------------------ the tree

def test_the_tree_is_clean(capsys):
    assert cdp.main([]) == 0
    assert ', 0 findings' in capsys.readouterr().out


def test_the_tree_lists_every_kind_of_protection_the_rules_name():
    entries, problems = cdp.parse_safety_tests(
        (cdp.ROOT / cdp.SAFETY_TESTS).read_text(encoding='utf-8'))
    assert problems == []
    assert {e.kind for e in entries} == cdp.KINDS


# ------------------------------------------------------------- reading lists

@pytest.mark.parametrize('text, says', [
    ('crates/a  a reason\n', 'before any section'),
    ('[drive]\ncrates/a  a reason\n', 'unknown section'),
    ('[drive path]\ncrates/a\n', 'has no reason'),
    ('[drive path]\ncrates/a/  a reason\n', 'no leading or trailing slash'),
])
def test_an_unreadable_drive_path_line_is_named(text, says):
    entries, findings = cdp.parse_drive_path(text)
    assert entries == []
    # an unknown section also leaves the lines under it in no section
    assert findings and says in findings[0]


def test_a_readable_drive_path_list_reads_with_its_comments():
    entries, findings = cdp.parse_drive_path(
        '# a comment\n[drive path]\ncrates/a  it drives  # and a note\n'
        '[not drive path]\ncrates/b  it reads\n')
    assert findings == []
    assert [(e.path, e.section, e.reason) for e in entries] == [
        ('crates/a', cdp.DRIVE, 'it drives'), ('crates/b', cdp.NOT_DRIVE, 'it reads')]


@pytest.mark.parametrize('line, says', [
    ('a src/lib.rs t\n', 'expected'),
    ('a src/lib.rs t drive-ceilings what\n', 'is not a kind'),
    ('a src/lib.rs t-x drive-ceiling what\n', 'is not a test name'),
    ('a src/lib.rs t drive-ceiling what\na src/lib.rs t drive-abort again\n',
     'already listed'),
])
def test_an_unreadable_safety_test_line_is_named(line, says):
    _, findings = cdp.parse_safety_tests(line)
    assert len(findings) == 1 and says in findings[0]


# ------------------------------------------------------------ rule 1: lists

def _lists_tree(root: pathlib.Path) -> None:
    _write(root, {'crates/a/Cargo.toml': '', 'crates/b/Cargo.toml': '',
                  'modules/m/Cargo.toml': '', 'schema/x.json': '{}', 'ui/app.ts': ''})


def test_lists_that_classify_everything_pass(tmp_path):
    _lists_tree(tmp_path)
    entries, _ = cdp.parse_drive_path(
        '[drive path]\ncrates/a r\nmodules/m r\n[not drive path]\ncrates/b r\n'
        'schema/x.json r\nui r\n')
    assert cdp.check_lists(entries, tmp_path) == []


def test_an_unclassified_crate_fails_and_says_where_to_put_it(tmp_path):
    _lists_tree(tmp_path)
    entries, _ = cdp.parse_drive_path(
        '[drive path]\ncrates/a r\nmodules/m r\n[not drive path]\nschema/x.json r\n')
    findings = cdp.check_lists(entries, tmp_path)
    assert len(findings) == 1
    assert findings[0].startswith('crates/b is on neither list') and '[drive path]' in findings[0]


def test_a_listed_path_that_does_not_exist_fails(tmp_path):
    _lists_tree(tmp_path)
    entries, _ = cdp.parse_drive_path(
        '[drive path]\ncrates/a r\ncrates/gone r\nmodules/m r\n'
        '[not drive path]\ncrates/b r\nschema/x.json r\n')
    findings = cdp.check_lists(entries, tmp_path)
    assert len(findings) == 1 and 'crates/gone does not exist' in findings[0]


def test_a_path_on_both_lists_fails(tmp_path):
    _lists_tree(tmp_path)
    entries, _ = cdp.parse_drive_path(
        '[drive path]\ncrates/a r\ncrates/b r\nmodules/m r\n'
        '[not drive path]\ncrates/b r\nschema/x.json r\n')
    findings = cdp.check_lists(entries, tmp_path)
    assert len(findings) == 1 and 'listed more than once' in findings[0]


def test_a_path_inside_another_listed_path_fails(tmp_path):
    _lists_tree(tmp_path)
    entries, _ = cdp.parse_drive_path(
        '[drive path]\ncrates r\ncrates/a r\nmodules/m r\n'
        '[not drive path]\nschema/x.json r\n')
    findings = cdp.check_lists(entries, tmp_path)
    assert len(findings) == 1 and 'contains crates/a' in findings[0]


# ---------------------------------------------------- rule 2: safety tests

GOOD = '''\
    #[cfg(test)]
    mod tests {
        // a brace in a comment: }
        const S: &str = "a brace in a string: } and r#\\"";
        const C: char = '}';

        #[test]
        fn holds_the_ceiling() {
            let s = r#"another } here"#;
            assert!(!s.is_empty());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn stops_on_a_dropped_controller() {}
    }
    '''


def test_a_present_test_passes_whatever_its_comments_and_strings_hold(tmp_path):
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', GOOD)})
    assert _check(tmp_path, '''\
        a src/lib.rs holds_the_ceiling drive-ceiling the ceiling
        a src/lib.rs tests::stops_on_a_dropped_controller dropped-controller the lease
        ''') == []


def test_a_missing_test_fails(tmp_path):
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', GOOD)})
    findings = _check(tmp_path, 'a src/lib.rs holds_the_floor drive-ceiling the ceiling\n')
    assert len(findings) == 1 and 'no function of that name' in findings[0]


def test_a_missing_file_or_crate_fails(tmp_path):
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', GOOD)})
    findings = _check(tmp_path, '''\
        a src/gone.rs holds_the_ceiling drive-ceiling the ceiling
        b src/lib.rs holds_the_ceiling drive-ceiling the ceiling
        ''')
    assert len(findings) == 2
    assert 'the file does not exist' in findings[0] and 'no crate crates/b' in findings[1]


def test_a_function_that_is_not_a_test_fails(tmp_path):
    lib = 'fn holds_the_ceiling() {}\n#[test]\nfn other() {}\n'
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', lib)})
    findings = _check(tmp_path, 'a src/lib.rs holds_the_ceiling drive-ceiling the ceiling\n')
    assert len(findings) == 1 and 'not marked as a test' in findings[0]


@pytest.mark.parametrize('attr', [
    '#[ignore]',
    '#[ignore = "slow"]',
    '#[cfg_attr(debug_assertions, ignore)]',
    '#[cfg_attr(windows, ignore = "flaky there")]',
])
def test_an_ignored_test_fails(tmp_path, attr):
    lib = f'#[test]\n{attr}\nfn holds_the_ceiling() {{}}\n'
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', lib)})
    findings = _check(tmp_path, 'a src/lib.rs holds_the_ceiling drive-ceiling the ceiling\n')
    assert len(findings) == 1 and 'is ignored' in findings[0]


@pytest.mark.parametrize('attr', ['#[cfg_attr(miri, ignore)]', '#[should_panic]',
                                  '#[cfg(windows)]', '#[cfg(not(target_os = "macos"))]'])
def test_an_attribute_that_does_not_drop_the_test_on_ci_passes(tmp_path, attr):
    lib = f'#[test]\n{attr}\nfn holds_the_ceiling() {{}}\n'
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', lib)})
    assert _check(tmp_path, 'a src/lib.rs holds_the_ceiling drive-ceiling the ceiling\n') == []


@pytest.mark.parametrize('lib', [
    '#[cfg(any())]\n#[test]\nfn holds_the_ceiling() {}\n',
    '#[test]\n#[cfg(all(unix, windows))]\nfn holds_the_ceiling() {}\n',
    '#[cfg(feature = "never")]\nmod tests {\n#[test]\nfn holds_the_ceiling() {}\n}\n',
    '#![cfg(loom)]\n#[test]\nfn holds_the_ceiling() {}\n',
])
def test_a_test_compiled_out_fails(tmp_path, lib):
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', lib)})
    findings = _check(tmp_path, 'a src/lib.rs holds_the_ceiling drive-ceiling the ceiling\n')
    assert len(findings) == 1 and 'leaves it out' in findings[0]


def test_two_tests_of_one_name_need_their_module(tmp_path):
    lib = ('mod one {\n#[test]\nfn stops() {}\n}\n'
           'mod two {\n#[test]\nfn stops() {}\n}\n')
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST, **_crate('a', lib)})
    findings = _check(tmp_path, 'a src/lib.rs stops stop-ramp the ramp\n')
    assert len(findings) == 1 and 'write module::name' in findings[0]
    assert _check(tmp_path, 'a src/lib.rs two::stops stop-ramp the ramp\n') == []


TEST_IN_FILE = '#[test]\nfn holds_the_ceiling() {}\n'


@pytest.mark.parametrize('lib, reachable', [
    ('mod service;\n', True),
    ('#[cfg(test)]\nmod service;\n', True),
    ('#[cfg(feature = "service")]\npub mod service;\n', False),
    ('mod other;\n', False),
])
def test_a_file_is_checked_through_the_mod_lines_that_reach_it(tmp_path, lib, reachable):
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST,
                      **_crate('a', lib, more={'src/service/mod.rs': 'mod engine;\n',
                                               'src/service/engine.rs': TEST_IN_FILE})})
    findings = _check(tmp_path,
                      'a src/service/engine.rs holds_the_ceiling drive-ceiling the ceiling\n')
    assert (findings == []) is reachable
    if not reachable:
        assert 'not compiled into the test build' in findings[0]


def test_a_feature_is_on_when_it_is_a_default_or_another_member_asks_for_it(tmp_path):
    lib = '#[cfg(feature = "service")]\npub mod service;\n'
    more = {'src/service.rs': TEST_IN_FILE}
    line = 'a src/service.rs holds_the_ceiling drive-ceiling the ceiling\n'
    # off: nobody enables it
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST,
                      **_crate('a', lib, '[features]\nservice = []\n', more)})
    assert len(_check(tmp_path, line)) == 1
    # a default
    _write(tmp_path, _crate('a', lib, '[features]\ndefault = ["service"]\nservice = []\n', more))
    assert _check(tmp_path, line) == []
    # asked for, through a feature of another member that is itself a default
    _write(tmp_path, {
        **_crate('a', lib, '[features]\nservice = []\n', more),
        **_crate('b', '', '[dependencies]\na = { path = "../a" }\n'
                          '[features]\ndefault = ["full"]\nfull = ["a/service"]\n')})
    assert _check(tmp_path, line) == []


@pytest.mark.parametrize('manifest, compiled', [
    ('', True),
    ('[[test]]\nname = "ramp"\nrequired-features = ["slow"]\n', False),
    (('[features]\ndefault = ["slow"]\nslow = []\n'
      '[[test]]\nname = "ramp"\nrequired-features = ["slow"]\n'), True),
    ('autotests = false\n', False),
])
def test_an_integration_test_target_is_checked(tmp_path, manifest, compiled):
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST,
                      **_crate('a', '', manifest, {'tests/ramp.rs': TEST_IN_FILE})})
    findings = _check(tmp_path, 'a tests/ramp.rs holds_the_ceiling drive-ceiling the ceiling\n')
    assert (findings == []) is compiled


def test_lib_test_false_leaves_the_crates_own_tests_out(tmp_path):
    _write(tmp_path, {'Cargo.toml': ROOT_MANIFEST,
                      **_crate('a', TEST_IN_FILE, '[lib]\ntest = false\n')})
    findings = _check(tmp_path, 'a src/lib.rs holds_the_ceiling drive-ceiling the ceiling\n')
    assert len(findings) == 1 and 'test = false' in findings[0]


@pytest.mark.parametrize('pred, linux, macos, windows', [
    ('unix', True, True, False),
    ('windows', False, False, True),
    ('target_os = "macos"', False, True, False),
    ('not(target_os = "linux")', False, True, True),
    ('all(unix, not(target_os = "macos"))', True, False, False),
    ('any()', False, False, False),
    ('all()', True, True, True),
    ('test', True, True, True),
    ('miri', False, False, False),
])
def test_cfg_predicates_are_read_per_platform(pred, linux, macos, windows):
    got = [cdp.eval_cfg(pred, p, frozenset()) for p in cdp.PLATFORMS]
    assert got == [linux, macos, windows]


def test_the_scanner_finds_a_functions_body_and_its_modules():
    scanned = cdp.scan_rust(textwrap.dedent('''\
        mod outer {
            mod inner {
                #[test]
                fn deep() {
                    let s = "}";
                    if true { }
                }
            }
            fn after() {}
        }
        '''))
    deep = next(f for f in scanned.fns if f.name == 'deep')
    after = next(f for f in scanned.fns if f.name == 'after')
    assert deep.modules == ('outer', 'inner') and (deep.line, deep.end) == (4, 7)
    assert after.modules == ('outer',) and after.attrs == []


# ------------------------------------------------------ rules 3 and 4: pull requests

def _git(root: pathlib.Path, *args: str) -> str:
    return subprocess.run(['git', *args], cwd=root, check=True, capture_output=True,
                          text=True).stdout


@pytest.fixture
def repo(tmp_path, monkeypatch):
    """A repository on `main` with one drive-path crate (a), one that is not
    (b), and one listed safety test; the test switches to a branch `pr`."""
    for key in ('AUTHOR', 'COMMITTER'):
        monkeypatch.setenv(f'GIT_{key}_NAME', 'test')
        monkeypatch.setenv(f'GIT_{key}_EMAIL', 'test@example.invalid')
    lib = '#[test]\nfn holds_the_ceiling() {\n    assert!(1 < 2);\n}\n'
    _write(tmp_path, {
        'Cargo.toml': ROOT_MANIFEST,
        **_crate('a', lib), **_crate('b', ''),
        cdp.DRIVE_PATH: '[drive path]\ncrates/a  it drives\n[not drive path]\ncrates/b  it reads\n',
        cdp.SAFETY_TESTS: 'a src/lib.rs holds_the_ceiling drive-ceiling the ceiling\n',
        cdp.RUN_DATA: '[run data]\ncrates/a/src/setup.rs  a setup\ndocs/format.md#run  the run\n',
        'crates/a/src/setup.rs': '// the setup\n',
        'docs/format.md': FORMAT,
    })
    _git(tmp_path, 'init', '-q', '-b', 'main')
    _git(tmp_path, 'add', '.')
    _git(tmp_path, 'commit', '-q', '-m', 'base')
    _git(tmp_path, 'checkout', '-q', '-b', 'pr')
    return tmp_path


def _commit(root: pathlib.Path, files: dict[str, str], remove: Iterable[str] = ()) -> None:
    _write(root, files)
    for name in remove:
        (root / name).unlink()
    _git(root, 'add', '-A')
    _git(root, 'commit', '-q', '-m', 'change')


def _pr(root: pathlib.Path, title: str = 'A change', body: str = '') -> tuple[list, list]:
    return cdp.check_pull_request(cdp.PullRequest('main', title, body), root)


def test_a_change_off_the_drive_path_needs_no_section(repo):
    _commit(repo, {'crates/b/src/lib.rs': '// a reader\n'})
    assert _pr(repo, body=TEMPLATE) == ([], [])
    assert _pr(repo, body='') == ([], [])


def test_a_drive_path_change_with_no_section_fails(repo):
    _commit(repo, {'crates/a/src/extra.rs': '// a ramp\n'})
    findings, _ = _pr(repo, body='## What and why\n\nA change.\n')
    assert len(findings) == 1
    assert 'crates/a/src/extra.rs' in findings[0] and 'has no "## Drive path' in findings[0]


def test_a_drive_path_change_with_the_template_untouched_fails_on_each_answer(repo):
    _commit(repo, {'crates/a/src/extra.rs': '// a ramp\n'})
    findings, _ = _pr(repo, body=TEMPLATE)
    assert len(findings) == 5
    assert 'says "No drive-path code touched."' in findings[0]
    assert all(f'answer "{label}"' in ' '.join(findings) or label in ' '.join(findings)
               for label in cdp.ANSWERS)


def test_a_drive_path_change_with_a_placeholder_answer_fails(repo):
    _commit(repo, {'crates/a/src/extra.rs': '// a ramp\n'})
    findings, _ = _pr(repo, body=FILLED.replace('the stop ramp suite, 40 runs.', 'TODO'))
    assert len(findings) == 1 and 'What ran on the simulator' in findings[0]


def test_a_drive_path_change_with_the_section_filled_passes(repo):
    _commit(repo, {'crates/a/src/extra.rs': '// a ramp\n'})
    assert _pr(repo, body=FILLED) == ([], [])
    # bold labels and an answer that runs onto the next line read the same
    bold = FILLED.replace('- What ran on hardware: Not on hardware.',
                          '- **What ran on hardware:**\n  Not on hardware.')
    assert _pr(repo, body=bold.replace('\n', '\r\n')) == ([], [])


def test_a_path_taken_off_the_list_still_counts_as_drive_path_and_needs_the_title(repo):
    _commit(repo, {cdp.DRIVE_PATH: '[not drive path]\ncrates/a  it reads now\ncrates/b  r\n',
                   'crates/a/src/extra.rs': '// a ramp\n'})
    findings, _ = _pr(repo, body=TEMPLATE)
    assert any('has no' in f or 'says "No drive-path' in f for f in findings)
    assert any('crates/a is no longer on the drive-path list' in f for f in findings)
    findings, _ = _pr(repo, title='Reclassify a [weakens safety]', body=FILLED)
    assert findings == []


def test_a_removed_safety_test_line_needs_the_title(repo):
    _commit(repo, {cdp.SAFETY_TESTS: '# nothing listed\n'})
    findings, _ = _pr(repo)
    assert len(findings) == 1
    assert 'holds_the_ceiling (drive-ceiling) is no longer in' in findings[0]
    assert '[weakens safety]' in findings[0]
    assert _pr(repo, title='[Weakens Safety] Retire a test')[0] == []


def test_a_test_moved_to_another_file_is_not_a_removal(repo):
    _commit(repo, {cdp.SAFETY_TESTS:
                   'a tests/ceiling.rs holds_the_ceiling drive-ceiling the ceiling\n',
                   'crates/a/tests/ceiling.rs': '#[test]\nfn holds_the_ceiling() {}\n',
                   'crates/a/src/lib.rs': ''})
    findings, _ = _pr(repo, body=FILLED)
    assert findings == []


def test_an_ignored_listed_test_fails_the_list_check_whatever_the_title(repo, capsys):
    _commit(repo, {'crates/a/src/lib.rs': '#[test]\n#[ignore]\nfn holds_the_ceiling() {}\n'})
    assert cdp.main([], root=repo) == 1
    assert 'is ignored' in capsys.readouterr().out


def test_an_edit_inside_a_listed_test_is_a_warning_not_a_finding(repo):
    _commit(repo, {'crates/a/src/lib.rs':
                   '#[test]\nfn holds_the_ceiling() {\n    assert!(1 < 3);\n}\n'})
    findings, warnings = _pr(repo, body=FILLED)
    assert findings == []
    assert len(warnings) == 1 and 'holds_the_ceiling (drive-ceiling) is edited' in warnings[0]


def test_a_listed_test_new_in_the_pull_request_is_not_flagged_as_edited(repo):
    _commit(repo, {cdp.SAFETY_TESTS:
                   'a src/lib.rs holds_the_ceiling drive-ceiling the ceiling\n'
                   'a src/ramp.rs ramps_down stop-ramp the ramp\n',
                   'crates/a/src/lib.rs': '#[test]\nfn holds_the_ceiling() {\n    assert!(1 < 2);\n}\n'
                                          'mod ramp;\n',
                   'crates/a/src/ramp.rs': '#[test]\nfn ramps_down() {}\n'})
    assert _pr(repo, body=FILLED) == ([], [])


def test_an_edit_beside_a_listed_test_is_not_flagged(repo):
    _commit(repo, {'crates/a/src/lib.rs':
                   '#[test]\nfn holds_the_ceiling() {\n    assert!(1 < 2);\n}\n'
                   'fn helper() {}\n'})
    assert _pr(repo, body=FILLED) == ([], [])


def _event(path: pathlib.Path, base: str, title: str, body: str | None) -> pathlib.Path:
    path.write_text(json.dumps({'pull_request': {
        'base': {'sha': base}, 'title': title, 'body': body}}), encoding='utf-8')
    return path


def test_main_reads_a_pull_request_event_and_exits_by_its_findings(repo, tmp_path_factory,
                                                                   capsys):
    _commit(repo, {'crates/a/src/extra.rs': '// a ramp\n'})
    base = _git(repo, 'rev-parse', 'main').strip()
    where = tmp_path_factory.mktemp('event')
    assert cdp.main(['--event', str(_event(where / 'e.json', base, 'A', None))],
                    root=repo) == 1
    assert 'has no "## Drive path' in capsys.readouterr().out
    assert cdp.main(['--event', str(_event(where / 'e.json', base, 'A', FILLED))],
                    root=repo) == 0


def test_main_on_an_event_that_is_not_a_pull_request_checks_the_lists_only(repo, tmp_path,
                                                                           capsys):
    event = tmp_path / 'push.json'
    event.write_text(json.dumps({'ref': 'refs/heads/main'}), encoding='utf-8')
    assert cdp.main(['--event', str(event)], root=repo) == 0
    assert 'not a pull request event' in capsys.readouterr().out


def test_main_says_so_when_git_cannot_answer(repo, capsys):
    assert cdp.main(['--base', 'no-such-branch'], root=repo) == 2
    assert 'could not run' in capsys.readouterr().out


def test_main_with_a_base_reads_the_title_and_the_body_file(repo, tmp_path):
    _commit(repo, {'crates/a/src/extra.rs': '// a ramp\n'})
    body = tmp_path / 'body.md'
    body.write_text(FILLED, encoding='utf-8')
    assert cdp.main(['--base', 'main', '--title', 'A', '--body-file', str(body)],
                    root=repo) == 0
    assert cdp.main(['--base', 'main', '--title', 'A'], root=repo) == 1


# ------------------------------------------------------- rule 5: run data

def test_a_hash_inside_a_path_is_an_anchor_and_after_a_space_a_comment():
    entries, findings = cdp.parse_run_data(
        '# a comment\n[run data]\ndocs/f.md#run  the run  # a note\n')
    assert findings == []
    assert [(e.path, e.reason) for e in entries] == [('docs/f.md#run', 'the run')]


def test_the_run_data_list_has_one_section():
    _, findings = cdp.parse_run_data('[drive path]\ncrates/a r\n')
    assert findings and 'the sections are [run data]' in findings[0]


@pytest.mark.parametrize('heading, anchor', [
    ('`channel_table/`', 'channel_table'),
    ('`run/`, a MIMO Random Run (Proposal)', 'run-a-mimo-random-run-proposal'),
    ('Hardware and Random Control', 'hardware-and-random-control'),
])
def test_a_heading_anchor_is_the_one_github_makes(heading, anchor):
    assert cdp.slug(heading) == anchor


def test_a_section_runs_to_the_next_heading_of_its_level_and_skips_code():
    text = FORMAT.replace('Samples.', '```\n## not a heading\n```')
    ranges = cdp.heading_ranges(text)
    # the fence adds two lines; `shapes/` starts at line 17
    assert ranges['run'] == (7, 16)
    assert ranges['its-streams'] == (11, 16)
    assert 'not-a-heading' not in ranges


def test_a_run_data_list_that_names_what_exists_passes(tmp_path):
    _write(tmp_path, {'docs/f.md': FORMAT, 'crates/a/src/x.rs': ''})
    entries, _ = cdp.parse_run_data('[run data]\ncrates/a r\ndocs/f.md#run r\n')
    assert cdp.check_run_data_list(entries, tmp_path) == []


@pytest.mark.parametrize('lines, says', [
    ('crates/gone r\n', 'crates/gone does not exist'),
    ('docs/f.md#nowhere r\n', 'no heading whose anchor is #nowhere'),
    ('docs/f.md#run r\ndocs/f.md#run again\n', 'already listed'),
])
def test_a_run_data_list_that_names_what_is_not_there_fails(tmp_path, lines, says):
    _write(tmp_path, {'docs/f.md': FORMAT})
    entries, _ = cdp.parse_run_data('[run data]\n' + lines)
    findings = cdp.check_run_data_list(entries, tmp_path)
    assert len(findings) == 1 and says in findings[0]


def test_a_change_to_a_listed_run_data_path_with_no_section_fails(repo):
    _commit(repo, {'crates/a/src/setup.rs': '// the setup, with a new field\n'})
    findings, _ = _pr(repo, body=FILLED)
    assert len(findings) == 1
    assert 'changes run data (crates/a/src/setup.rs)' in findings[0]
    assert 'has no "## Run data" section' in findings[0]


def test_a_run_data_change_with_the_template_untouched_fails_on_each_field(repo):
    _commit(repo, {'crates/a/src/setup.rs': '// the setup, with a new field\n'})
    findings, _ = _pr(repo, body=FILLED + RUN_TEMPLATE)
    assert len(findings) == 1 + len(cdp.RUN_SECTION.answers)
    assert 'says "No run data touched."' in findings[0]
    for label in cdp.RUN_SECTION.answers:
        assert any(f'answer "{label}"' in f for f in findings)


def test_a_run_data_change_with_a_placeholder_field_fails(repo):
    _commit(repo, {'crates/a/src/setup.rs': '// the setup, with a new field\n'})
    body = FILLED + RUN_FILLED.replace('row not possible yet', 'TBD')
    findings, _ = _pr(repo, body=body)
    assert len(findings) == 1 and '"Bridge test"' in findings[0]


def test_a_run_data_change_with_the_section_filled_passes_and_none_is_an_answer(repo):
    _commit(repo, {'crates/a/src/setup.rs': '// the setup, with a new field\n'})
    assert _pr(repo, body=FILLED + RUN_FILLED) == ([], [])


def test_an_edit_under_a_listed_heading_is_run_data(repo):
    _commit(repo, {'docs/format.md': FORMAT.replace('Samples.', 'Samples, with units.')})
    findings, _ = _pr(repo, body=TEMPLATE)
    assert len(findings) == 1 and 'changes run data (docs/format.md#run)' in findings[0]
    assert _pr(repo, body=RUN_FILLED) == ([], [])


@pytest.mark.parametrize('old, new', [
    ('Points.', 'Points, and lines.'),
    ('Modes.', 'Modes and their damping.'),
])
def test_an_edit_under_another_heading_of_a_listed_file_is_not(repo, old, new):
    _commit(repo, {'docs/format.md': FORMAT.replace(old, new)})
    assert _pr(repo, body='') == ([], [])


def test_a_section_removed_whole_is_still_run_data(repo):
    _commit(repo, {'docs/format.md': FORMAT.replace('## `run/`\n\nA run.\n\n### Its Streams'
                                                    '\n\nSamples.\n\n', '')})
    findings, _ = _pr(repo, body='')
    assert len(findings) == 1 and 'docs/format.md#run' in findings[0]


def test_main_fails_without_the_run_data_list(repo, capsys):
    (repo / cdp.RUN_DATA).unlink()
    assert cdp.main([], root=repo) == 1
    assert f'{cdp.RUN_DATA} must exist' in capsys.readouterr().out
