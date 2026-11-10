# The Desktop App

`desktop/` is the Hertzforge desktop app: a Tauri 2 window that shows the shell (`ui/`), with the
control service and the analysis service started beside it. Nothing else needs installing: the app
carries its own Python. The browser setup (`scripts/dev.sh`, `?mock=1`) stays as it was and is for
development; `scripts/dev_app.sh` runs the app itself from the checkout (Trying a Change in the App).

## What the Installers Contain

| Part | Where it is in the app | What it is |
|---|---|---|
| The window | the app's executable (`hertzforge`) | Tauri 2; the shell from `ui/build`, served from inside the app |
| The control service | `sds-control`, beside the executable | the release build of `crates/sds-control` |
| The analysis service's Python | `python/` in the app's resources | CPython 3.13 from python-build-standalone, with the `hertzforge` package and its dependencies installed as published wheels |
| License texts | `LICENSE`, `THIRD_PARTY_NOTICES.md` in the resources | |

Built on an Apple silicon Mac (November 2, 2026): the app is 541 MB and its disk image 168 MB. The
Python runtime is 471 MB of that, mostly the solid-modeling kernel's wheel (225 MB), SciPy (66 MB),
pandas (37 MB) and netCDF4 (27 MB). The 55 MB of plate test data the app carried until November 5
is not in it: the example projects are downloads on hertzforge.com (`scripts/make_examples.py`
writes them).

The CI workflow `.github/workflows/desktop.yml` builds on every push to the desktop branch and on
demand: a `.dmg` each for macOS on Apple silicon and on Intel, an NSIS installer (`*-setup.exe`) for Windows x64, and
a `.deb` and an AppImage for Linux x64, as workflow artifacts kept for 14 days. It is not a required
check.

## Trying a Change in the App

`scripts/dev_app.sh` opens the app from the checkout without packaging it: the real window, menus
and file dialogs (Tauri's development mode), the shell from its development server, and both
services from the checkout. It builds the control service (release, only what changed), starts the
shell's development server on port 5191 unless one is already there, and runs `tauri dev` with
`HERTZFORGE_DEV_CONTROL` and `HERTZFORGE_DEV_PYTHON` naming the two services. Only a development
build reads those; a release build starts the services it carries, whatever the environment says.

```bash
scripts/dev_app.sh
```

A change saved under `ui/` shows in the window at once. A change to the Python package or to the
control service shows the next time it runs: quit the app and run it again, a matter of seconds once
the first run (a few minutes) has built everything. A change under `desktop/` rebuilds and restarts
the app by itself. The analysis service runs on the Python `python3` resolves to in the checkout,
which must have the package installed for development (`pip install -e .`); `HERTZFORGE_DEV_PYTHON`
names another. The script resolves a pyenv shim to the interpreter itself, since the app starts the
service from another folder, where the shim would choose another Python.

On a Mac, a launcher in `~/Applications` can run it in a Terminal window, which shows the build and
the services' output; quitting the app ends it. It is an AppleScript applet,
`tell application "Terminal" to do script "cd ~/hertzforge && scripts/dev_app.sh"`, compiled with
`osacompile`.

## Building It Locally

You need the Rust toolchain `rust-toolchain.toml` names, Node.js (LTS), `curl`, and on Linux the
libraries Tauri builds against (the workflow's apt line lists them). Then, from the repository root:

```bash
CARGO_TARGET_DIR=<a build folder> desktop/packaging/build.sh
```

It builds the shell, the control service (release), the Python runtime
(`desktop/packaging/bundle_python.sh`) and then the app and its installers with the Tauri command
line (`desktop/package.json`). The installers land in `<build folder>/release/bundle/`; on macOS
`--bundles app,dmg` makes only the app and the disk image. `HERTZFORGE_KEEP_PYTHON=1` reuses the
runtime staged by an earlier build, which saves the download.

The desktop crate is a Cargo workspace of its own, so the controller's `cargo test --workspace` and
`cargo clippy` do not compile the window toolkit. Its own checks:

```bash
npm --prefix desktop ci && npm --prefix desktop run icons   # the icons, from desktop/icon.png
cargo clippy --manifest-path desktop/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path desktop/Cargo.toml --locked
cargo deny --manifest-path desktop/Cargo.toml --config desktop/deny.toml check
```

## The Python It Carries

The analysis service runs on a relocatable CPython from python-build-standalone, with the package
and its dependencies installed into it as published wheels, started as
`python -I -B -m hertzforge serve`: the same command a developer runs, isolated from any Python the
computer has (`-I`), and never writing bytecode into the installed app (`-B`; the runtime's bytecode
is compiled at build time, checked by hash). The interpreter build is named in `bundle_python.sh`
and checked against its release's checksums.

The other route, a PyInstaller freeze, was not taken: PyInstaller's
bootloader is GPL-2.0 with an exception for the programs it starts, which would need a ruling under
this project's license policy, and a freeze adds its own analysis and hooks to keep working. Both
routes carry the same wheels, so the size is about the same either way.

Measured on an Apple silicon Mac: the analysis service from the bundled runtime listens 1.2 s after
it starts (its heavy imports load first), and the app, opened from a fresh copy, had both services
listening 3.8 s after it was opened and its window up at 4.0 s.

## The Sparse Solver

The modal solve factors its matrices with the fastest solver the machine has
(`core/sparse_solve.py`): Apple's Accelerate on a Mac, Intel's PARDISO through oneMKL on x86-64
Windows and Linux (`pypardiso`, a dependency there), and SuperLU anywhere else. Intel's libraries are proprietary, the one exception to the permissive rule
(`THIRD_PARTY_NOTICES.md`). The bundle keeps only the libraries PARDISO loads, threaded through TBB,
and `bundle_python.sh` runs `hertzforge check-solver` on the bundled runtime before it is packed,
so a build that fell back to SuperLU fails rather than ships. CI runs the same check on every
runner. A Windows model of 193,627 bricks that took 70 minutes through SuperLU
took four through PARDISO. The sizes the trimming leaves in Hertzforge's installers are not
measured yet.

## How the Shell Finds the Services

The app starts both services with `--port 0`, so the operating system picks a free loopback port
(127.0.0.1 only), and reads each service's "listening on ws://127.0.0.1:<port>/" line from its log.
It then opens the window at the shell's own address with the two addresses in the query
(`?analysis=ws://...&control=ws://...`), which is how the shell already reads them in a browser
(`ui/src/lib/app/config.ts`). A display page opened in a window of its own copies the query, so it
reaches the same services. The window's content security policy allows connections to the app
itself and to loopback only.

## Stopping a Run When the App Quits

A killed control service can leave an NI output holding its last sample, so the app never kills it.
Each service's standard input is a pipe the app holds and never writes to; each service is started
with `--watch-stdin`.

- **Quitting** (closing the main window, or Quit in the menu): if a Random test or a simulation is
  live, the app asks first ("Stop the Test and Quit?", with the test named), and Keep Running
  leaves everything as it was. Then the app closes both pipes. The control service refuses anything
  that could start or raise a drive, ends the run with its controlled stop (the drive ramps down
  over the test's shutdown ramp, the same path as the Stop button; never an abort), and exits once
  the run has ended. The window shows the ramp-down meanwhile. The app waits for the control
  service to exit before it exits itself. After five minutes it exits anyway and leaves the service
  to finish its stop on its own; it still does not kill it.
- **If the app dies** (a crash, or the operating system ending it), the operating system closes the
  pipes, and the control service does the same as above on its own. On macOS the Dock's Quit and
  logging out end the app this way, without the question.
- The run's outcome is `safe_stop`, and its message says the control service was shut down.
- The control service does the same on Ctrl-C, SIGTERM or SIGHUP (and on Windows on Ctrl-Break or a
  closed console), so stopping `scripts/dev.sh` during a run also takes the controlled stop
  (`schema/PROTOCOL.md`, "Shutting Down").

The analysis service drives nothing. It exits when its pipe closes, and the app ends it if it has
not exited ten seconds after.

Logs of each launch are kept (the last ten): on macOS in `~/Library/Logs/org.hertzforge.app/`, with
`app.log`, `control.log` and `analysis.log`. The analysis service's cache is in the app's cache
folder (`~/Library/Caches/org.hertzforge.app/` on macOS). The control service's data root stays
`<home>/Hertzforge_Data`.

## Unsigned Builds

The Windows and Linux builds are not signed, and neither is a Mac build from `build.sh` alone. What a
user sees because of it:

- **macOS.** An unsigned build is signed ad hoc (no developer identity), which Apple silicon requires
  before it will start an app at all. A downloaded copy is refused the first time it is opened, with a
  warning that it could not be verified; the user allows it once under System Settings, Privacy &
  Security, Open Anyway (on older macOS versions, Open from the Finder's context menu does the same).
  The images CI makes are signed and notarized, so this does not apply to them.
- **Windows.** SmartScreen warns about an unrecognized app; More info, then Run anyway, installs it.
  The installer is per user, with no administrator prompt. The app uses the system's WebView2
  runtime, present on Windows 11 and current Windows 10; where it is missing, the installer
  downloads it, which needs a network that one time.
- **Linux.** Nothing is signed; the AppImage needs to be made executable.

On the developer's Mac, `desktop/packaging/sign_macos.sh` signs every binary in
the app with the hardened runtime (`entitlements.plist`), notarizes and staples the app, then makes,
signs and notarizes the disk image. It finds
its credentials: the "Developer ID Application" identity in the keychain, and for notarization the
app-specific password in the login-keychain item `vd-notary-password` (or the notarytool profile
`vd-notary`). The team is the one the identity's name ends with. After a build:

```bash
desktop/packaging/sign_macos.sh "$CARGO_TARGET_DIR/release/bundle/macos/Hertzforge.app" Hertzforge.dmg
```

The first run, on November 3, 2026, signed and notarized the app and its image on the first try, and
Gatekeeper assessed both as "Notarized Developer ID". The signed app started both services, and its
bundled Python, under the hardened runtime with `entitlements.plist`, imported NumPy, SciPy,
pandas, netCDF4, h5py and the CAD kernel and computed a PSD from the plate data. The signed app is not
yet tried as a copy downloaded on another Mac.

## Signing on the Runners

The desktop workflow builds the macOS images on GitHub's runners, one for Apple silicon and one for
Intel, signs and notarizes them with `sign_macos.sh`, and fails if Apple has not accepted both the app
and the image. Pushing a tag `v*` also publishes a GitHub release with the two images, the Windows and
Linux installers, and their `SHA256SUMS` on it, once every one of them has built, and then starts
`release.yml`. A run started by hand on main builds the same images as a test and leaves them as
artifacts for 14 days. A push to `feature/desktop-app` on a maintainer's fork still builds the images,
unsigned, because a fork has no certificate. Windows and Linux stay unsigned;
their installers are on the release beside the Mac images.

The credentials are secrets of the `release` environment, which admits only `v*` tags and main, so a
workflow on another branch cannot read them. An admin of the repository can change that rule. The
maintainer who owns the certificate sets them from their own terminal; no agent handles them:

- `MACOS_CERTIFICATE`: the Developer ID Application certificate with its private key, exported from
  Keychain Access as a `.p12` with a password, in base64;
- `MACOS_CERTIFICATE_PASSWORD`: that password;
- `NOTARY_APPLE_ID` and `NOTARY_PASSWORD`: the Apple ID and an app-specific password made for the
  runners alone, so it can be revoked without touching the one on the developer's Mac;
- `NOTARY_TEAM_ID`: the team the certificate belongs to.

`desktop/packaging/ci_signing.sh` puts the certificate in a keychain of the job's own, and the runner is
thrown away after the job.

## The Local Proof

The `e2e` feature (never in a shipped build) runs a script in the app's window that drives the shell
through its test ids, and answers the quit question, which nothing can click:

```bash
CARGO_TARGET_DIR=<a build folder> desktop/packaging/build.sh --e2e --bundles app
python scripts/make_examples.py <a folder> --only plate-random-run free-plate
HERTZFORGE_E2E_SCRIPT=desktop/packaging/e2e/random_run.js HERTZFORGE_E2E_ANSWER=stop \
  HERTZFORGE_E2E_FILES="<a folder>/plate-random-run.hforge:<a folder>/free-plate.hforge" \
  "<a build folder>/release/bundle/macos/Hertzforge.app/Contents/MacOS/hertzforge"
```

`desktop/packaging/e2e/random_run.js` checks both services connected and nothing loaded from the
network; drops the two example projects on the window (`HERTZFORGE_E2E_FILES`, separated as PATH
is) and shows the run's PSDs in Random and the model's modes in Modal; runs Random on the
simulator to completion from Acquire & Control; opens a display page in a window of its own; then
starts a second run and quits while it drives. Each step is written to the app's log, and the control
service's log says how the run ended. `desktop/packaging/e2e/drop_files.js` checks that the shell
sees Tauri's API and that a drop on the window (sent as the `tauri://drag-drop` event the window
emits for one) reaches the analysis service by its path. Build the app again without `--e2e` before
handing it on.

## Releasing

A release is a tag. Set the version in `pyproject.toml`, `src/hertzforge/__init__.py`,
`desktop/tauri.conf.json`, `desktop/package.json` and the workspace `Cargo.toml` to the same number,
merge, then tag that commit `v<version>` and push the tag. The desktop workflow builds, signs and
notarizes both Mac images, builds the Windows and Linux installers, renames them
`Hertzforge-macOS-AppleSilicon.dmg`, `Hertzforge-macOS-Intel.dmg`, `Hertzforge-Windows-x64-setup.exe`,
`Hertzforge-Linux-x64.AppImage` and `Hertzforge-Linux-x64.deb`, and publishes the release with them, their
`SHA256SUMS` and, when the updater key is set, the update archives and `latest.json`. It waits for
every installer, so a failed one publishes nothing. It then starts `release.yml` on the tag, which
tests the Python package and publishes it to PyPI. A release made with a workflow's own token starts
no other workflow, which is why the desktop workflow starts it by name. The site's Download section
links the images through `releases/latest/download/<name>`, which GitHub answers with the newest
published release's asset of that name, so the page needs no change per release; its script fills
the version, date and sizes from the GitHub API when the visitor's browser can reach it.

## Updates

Check for Updates, in the shell's command palette, asks the Tauri updater: it reads the latest
release's `latest.json`, says through a native dialog whether a newer version is published, and on
Install downloads that release's `Hertzforge-macOS-<Arch>.app.tar.gz`, verifies it against the
public key in `desktop/tauri.conf.json` (`plugins.updater.pubkey`), replaces the app and relaunches.
Only when asked, never at startup. The archive is the signed, stapled app
the same run notarized, tarred after signing and signed again with the updater's key by
`tauri signer sign`; `desktop/packaging/updater_manifest.py` writes the manifest beside it.

The updater's key pair is made once by the maintainer, from their own terminal, and no agent handles
the private half:

```bash
npm --prefix desktop exec -- tauri signer generate -w ~/.tauri/hertzforge.key
```

The public key it prints goes in `desktop/tauri.conf.json` as `plugins.updater.pubkey`; the private
key's file contents and its password go in the `release` environment as
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Until they are set, a release
carries images and no update archive, and Check for Updates says no update is published. An app can
only update to a release made after its own, so the first release with the key set is the one every
later app updates from.

## Not Done Yet

- The Windows and Linux installers are built by CI and have not been installed and run.
- The `.hforge` file association. On macOS the File menu has Open, Import, Save and Save As; on Windows
  and Linux, which have no menu bar yet, those are buttons in the top bar. They use the native dialogs, and
  files dropped on the window open or import by their paths (`capabilities/main.json` grants the
  main window its events and the two dialogs; the display windows get nothing).
- Windows on Arm.
- Updates on Windows and Linux: the updater is wired for the macOS app only.
