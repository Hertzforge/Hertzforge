import pytest

from hertzforge import __version__
from hertzforge.__main__ import main


def test_version(capsys):
    with pytest.raises(SystemExit) as exit_info:
        main(["--version"])
    assert exit_info.value.code == 0
    assert capsys.readouterr().out.strip() == f"hertzforge {__version__}"


def test_no_arguments_prints_help(capsys):
    main([])
    assert "usage: hertzforge" in capsys.readouterr().out


def test_serve_is_the_analysis_services_own_command(capsys):
    with pytest.raises(SystemExit) as exit_info:
        main(["serve", "--help"])
    assert exit_info.value.code == 0
    out = capsys.readouterr().out
    assert "usage: hertzforge serve" in out
    assert "--port" in out


def test_an_unknown_argument_is_refused(capsys):
    with pytest.raises(SystemExit) as exit_info:
        main(["--colour"])
    assert exit_info.value.code == 2


def test_check_solver_names_the_backend_and_exits_with_its_verdict(capsys, monkeypatch):
    """`hertzforge check-solver` is how a packaged build proves it carries
    its fast solver: the backend named on stdout, 0 when it is the one this
    machine should have, 1 otherwise."""
    from hertzforge.core import sparse_solve

    assert main(["check-solver"]) == 0
    out = capsys.readouterr().out
    assert out.startswith(sparse_solve.expected_backend()) and "modes below 1.0" in out
    monkeypatch.setattr(sparse_solve, "check", lambda: (1, "expected pardiso, got superlu"))
    assert main(["check-solver"]) == 1
    assert "expected pardiso, got superlu" in capsys.readouterr().out
