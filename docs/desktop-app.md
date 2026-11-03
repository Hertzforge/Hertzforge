# The Desktop App

`desktop/` is the Hertzforge desktop app: a Tauri 2 window that shows the shell (`ui/`), with the
control service and the analysis service started beside it. Nothing else needs installing: the app
carries its own Python. The browser setup (`scripts/dev.sh`, `?mock=1`) stays as it was and is for
development.

## What the Installers Contain

| Part | Where it is in the app | What it is |
|---|---|---|
| The window | the app's executable (`hertzforge`) | Tauri 2; the shell from `ui/build`, served from inside the app |
| The control service | `sds-control`, beside the executable | the release build of `crates/sds-control` |
| The analysis service's Python | `python/` in the app's resources | CPython 3.13 from python-build-standalone, with the `hertzforge` package and its dependencies installed as published wheels |
| The demonstration data | `testdata/plate/` in the resources | the plate runs and models the catalog's datasets read (`scripts/fetch_testdata.sh`) |
| License texts | `LICENSE`, `THIRD_PARTY_NOTICES.md` in the resources | |

Built on an Apple silicon Mac (November 2, 2026): the app is 541 MB and its disk image 168 MB. The
Python runtime is 471 MB of that, mostly the solid-modeling kernel's wheel (225 MB), SciPy (66 MB),
pandas (37 MB) and netCDF4 (27 MB); the plate data is 55 MB.

The CI workflow `.github/workflows/desktop.yml` builds on every push to the desktop branch and on
demand: a `.dmg` for macOS on Apple silicon, an NSIS installer (`*-setup.exe`) for Windows x64, and
a `.deb` and an AppImage for Linux x64, as workflow artifacts kept for 14 days. It is not a required
check.

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
npm --prefix desktop ci && npm --prefix desktop run icons   # the icons, from ui/static/favicon.svg
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

The other route, a PyInstaller freeze as Visual Dynamics ships, was not taken: PyInstaller's
bootloader is GPL-2.0 with an exception for the programs it starts, which would need a ruling under
this project's license policy, and a freeze adds its own analysis and hooks to keep working. Both
routes carry the same wheels, so the size is about the same either way.

Measured on an Apple silicon Mac: the analysis service from the bundled runtime listens 1.2 s after
it starts (its heavy imports load first), and the app, opened from a fresh copy, had both services
listening 3.8 s after it was opened and its window up at 4.0 s.

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

The builds are not signed yet. What a user sees because of it:

- **macOS.** The app is signed ad hoc (no developer identity), which Apple silicon requires before it
  will start an app at all. A downloaded copy is refused the first time it is opened, with a warning
  that it could not be verified; the user allows it once under System Settings, Privacy & Security,
  Open Anyway (on older macOS versions, Open from the Finder's context menu does the same). This was
  not tried on a downloaded copy: the copy tested here was built on the same Mac.
- **Windows.** SmartScreen warns about an unrecognized app; More info, then Run anyway, installs it.
  The installer is per user, with no administrator prompt. The app uses the system's WebView2
  runtime, present on Windows 11 and current Windows 10; where it is missing, the installer
  downloads it, which needs a network that one time.
- **Linux.** Nothing is signed; the AppImage needs to be made executable.

Signing for macOS is prepared and not enabled. `desktop/packaging/ci_signing.sh` puts a Developer ID
certificate from repository secrets into a keychain of the runner's own, and
`desktop/packaging/sign_macos.sh` signs every binary in the app with the hardened runtime
(`entitlements.plist`), notarizes and staples the app, then makes, signs and notarizes the disk image.
Both are adapted from Visual Dynamics' signed release route. To enable them: set the secrets
`MACOS_CERTIFICATE` (the .p12 with its private key, base64), `MACOS_CERTIFICATE_PASSWORD`,
`NOTARY_APPLE_ID`, `NOTARY_PASSWORD` (an app-specific password) and `NOTARY_TEAM_ID`, and the
repository variable `HERTZFORGE_SIGN_MACOS` to `true`. The first signed build should be launched
from its downloaded image before it is trusted: the entitlements the bundled Python needs under the
hardened runtime are Visual Dynamics', not yet tried here. Windows stays unsigned, as Visual Dynamics'
builds are.

## The Local Proof

The `e2e` feature (never in a shipped build) runs a script in the app's window that drives the shell
through its test ids, and answers the quit question, which nothing can click:

```bash
CARGO_TARGET_DIR=<a build folder> desktop/packaging/build.sh --e2e --bundles app
HERTZFORGE_E2E_SCRIPT=desktop/packaging/e2e/random_run.js HERTZFORGE_E2E_ANSWER=stop \
  "<a build folder>/release/bundle/macos/Hertzforge.app/Contents/MacOS/hertzforge"
```

`desktop/packaging/e2e/random_run.js` checks both services connected and nothing loaded from the
network; opens a dataset and runs a tool in Analyze and solves a model in Modal; runs Random on the
simulator to completion from Acquire & Control; opens a display page in a window of its own; then
starts a second run and quits while it drives. Each step is written to the app's log, and the control
service's log says how the run ended. Build the app again without `--e2e` before handing it on.

## Not Done Yet

- Signing, as above; notarization has not been tried.
- The Windows and Linux installers are built by CI and have not been installed and run.
- File dialogs, File menu items, drag and drop and the `.hforge` file association: the macOS menu
  has the standard application, Edit, View and Window menus only.
- Intel Macs and Windows on Arm.
- Updates: there is no updater.
