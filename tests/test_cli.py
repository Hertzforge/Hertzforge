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
