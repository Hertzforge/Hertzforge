"""The ``hertzforge`` command: ``hertzforge --version`` prints the version."""

import argparse

from hertzforge import __version__


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="hertzforge",
        description="Hertzforge: data acquisition and structural dynamics in one toolset.",
        epilog="This release has no analysis features yet. https://github.com/hertzforge/hertzforge",
    )
    parser.add_argument("--version", action="version", version=f"hertzforge {__version__}")
    parser.parse_args(argv)
    parser.print_help()


if __name__ == "__main__":
    main()
