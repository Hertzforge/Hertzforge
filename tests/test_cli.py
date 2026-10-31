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
