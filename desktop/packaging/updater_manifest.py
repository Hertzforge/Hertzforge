"""Write the manifest the app's updater reads, `latest.json`, beside a
release's update archives.

    python3 desktop/packaging/updater_manifest.py v0.1.0 images/

`images/` holds what the draft job renamed: `Hertzforge-macOS-<Arch>.app.tar.gz`
and its `.sig`, the signature `tauri signer sign` wrote with the release's
key. The manifest names each archive by the Tauri platform key the app
asks for and carries its signature inline, as the Tauri updater reads it:

    {"version": "0.1.0", "pub_date": "...", "notes": "",
     "platforms": {"darwin-aarch64": {"url": "...", "signature": "..."}, ...}}

With no archive beside it (the signing key not set when the images were
built) nothing is written, and the app's Check for Updates says no
update is published. Every published release's `latest.json` is what
`releases/latest/download/latest.json` serves, the one URL the app reads.
"""
from __future__ import annotations

import datetime as dt
import json
import sys
from pathlib import Path

REPO = 'https://github.com/Hertzforge/Hertzforge'
ARCHIVES = {'darwin-aarch64': 'Hertzforge-macOS-AppleSilicon.app.tar.gz',
            'darwin-x86_64': 'Hertzforge-macOS-Intel.app.tar.gz'}


def main(tag: str, folder: str) -> int:
    images = Path(folder)
    platforms = {}
    for key, name in ARCHIVES.items():
        signature = images / f'{name}.sig'
        if (images / name).is_file() and signature.is_file():
            platforms[key] = {'url': f'{REPO}/releases/download/{tag}/{name}',
                              'signature': signature.read_text().strip()}
    if not platforms:
        print('no signed update archive: no latest.json')
        return 0
    manifest = {'version': tag.removeprefix('v'),
                'pub_date': dt.datetime.now(dt.UTC).strftime('%Y-%m-%dT%H:%M:%SZ'),
                'notes': f'{REPO}/releases/tag/{tag}',
                'platforms': platforms}
    (images / 'latest.json').write_text(json.dumps(manifest, indent=1) + '\n')
    print(f'latest.json: {", ".join(platforms)}')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1], sys.argv[2]))
