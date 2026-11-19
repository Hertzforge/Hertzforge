import pytest

from hertzforge import __version__
from hertzforge.__main__ import main


def test_version(capsys):
    with pytest.raises(SystemExit) as exit_info:
        main(["--version"])
    assert exit_info.value.code == 0
    assert capsys.readouterr().out.strip() == f"hertzforge {__version__}"


def test_help_says_no_command_opens_the_interface(capsys):
    with pytest.raises(SystemExit):
        main(["--help"])
    # argparse wraps the description to the terminal's width
    out = " ".join(capsys.readouterr().out.split())
    assert "With no command, it opens the interface in a browser." in out


def test_no_command_without_a_built_interface_says_where_to_get_one(capsys, monkeypatch):
    """A source checkout that never built the shell has nothing to serve:
    the command says how to build it or where the app is, and exits 1
    without starting the service."""
    import hertzforge.service.__main__ as entry

    monkeypatch.setattr("hertzforge.service.server.shell_folder", lambda: None)
    monkeypatch.setattr(entry, "_serve", lambda *a, **k: pytest.fail("the service started"))
    assert main([]) == 1
    out = capsys.readouterr().out
    assert "carries no interface" in out
    assert "npm --prefix ui run build" in out


def test_no_command_starts_the_service_and_opens_its_page(capsys, monkeypatch, tmp_path):
    """`hertzforge` alone serves on 8791 when it is free and opens the page
    that points the shell at that port with no control service."""
    import hertzforge.__main__ as command
    import hertzforge.service.__main__ as entry

    (tmp_path / "index.html").write_text("<html></html>")
    monkeypatch.setattr("hertzforge.service.server.shell_folder", lambda: tmp_path)
    monkeypatch.setattr(command, "_free", lambda port: True)
    opened = []
    monkeypatch.setattr("webbrowser.open", lambda url: opened.append(url) or True)
    served = {}

    async def serve(host, port, *, ready=None, **_):
        served.update(host=host, port=port)
        ready(host, port)

    monkeypatch.setattr(entry, "_serve", serve)
    assert main([]) == 0
    assert served == {"host": "127.0.0.1", "port": 8791}
    assert opened == ["http://127.0.0.1:8791/?analysis=ws://127.0.0.1:8791/&control=none"]
    assert "Hertzforge is open at http://127.0.0.1:8791/" in capsys.readouterr().out


def test_no_command_picks_another_port_when_8791_is_taken(monkeypatch, tmp_path):
    """The desktop app holds 8791 while it runs; the command then lets the
    system pick a port rather than failing, and the page names it."""
    import socket

    import hertzforge.__main__ as command
    import hertzforge.service.__main__ as entry

    with socket.socket() as holder:
        holder.bind(("127.0.0.1", 0))
        holder.listen()
        taken = holder.getsockname()[1]
        assert not command._free(taken)
    (tmp_path / "index.html").write_text("<html></html>")
    monkeypatch.setattr("hertzforge.service.server.shell_folder", lambda: tmp_path)
    monkeypatch.setattr(command, "_free", lambda port: False)
    monkeypatch.setattr("webbrowser.open", lambda url: True)
    asked = []

    async def serve(host, port, *, ready=None, **_):
        asked.append(port)

    monkeypatch.setattr(entry, "_serve", serve)
    main([])
    assert asked == [0]


def test_where_no_browser_opens_the_address_is_given_to_open_by_hand(capsys):
    from hertzforge.__main__ import _announce

    _announce("http://127.0.0.1:5/?analysis=ws://127.0.0.1:5/&control=none", lambda url: False)
    out = capsys.readouterr().out
    assert "Hertzforge is open at http://127.0.0.1:5/" in out
    assert "No browser could be opened here; open that address in one." in out


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
