"""A geometry's animation recorded to a video with no window open (`hertzforge record`).

The animation is the one the app's 3-D view plays for the same selection (`service.show`):
a shape set's modes on their geometry, or data played on it, measured motion or modal
coordinates through their shape set. It is drawn by the app's own 3-D view, in the report
viewer's page (`report/viewer.html`) with a recording in place of a report, so it looks as the
app does. Headless Chrome draws the page; each frame is drawn on demand at its own time, not as
the page plays, so a slow machine records the same video as a fast one; ffmpeg writes the
frames to an H.264 MP4.

Chrome (or Chromium, or Edge) and ffmpeg are found on the machine, or named by the
HERTZFORGE_CHROME and HERTZFORGE_FFMPEG variables. Nothing else is needed: Chrome is driven
over its DevTools protocol with `websockets`, which the service already uses.
"""

from __future__ import annotations

import base64
import json
import os
import shutil
import subprocess
import tempfile
import time
import urllib.request
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .animation import AnimationError, amplitude_unit, framed, opening_camera, staged
from .animation import animation_view as _shown

#: seconds a mode takes to swing once, as the app plays it (`Shape3DPane` CYCLE_S)
CYCLE_S = 1.6

#: how long the page may take to draw the view before the recording gives up
READY_S = 120.0


class RecordError(AnimationError):
    """A recording that cannot be made, in words for the person running it."""


@dataclass
class Recording:
    """What a recording is made of, and how it is drawn and timed.

    `geometry` is drawn; with `data` that data plays on it (modal coordinates through `shapes`,
    or their own set when the project knows it); with only `shapes`, its mode `mode` (from 1)
    swings. `speed` is the fraction of real time measured motion plays at, or `seconds` the
    video's length; a mode swings for `cycles` cycles unless `seconds` says otherwise. `scale`
    multiplies the app's own amplitude (the largest deflection drawn at 7.5 % of the model's
    size). `opacity` is how solid the surface is drawn, 1 solid. `view` names a camera from
    `animation.VIEWS` ('iso' the geometry's own view), or `eye` and `up` give one. `frame` names
    element groups of the geometry to fit the camera to, in place of the whole model: the rest
    is still drawn, running out of the frame, and the amplitude is still a share of the whole
    model's size, so framing closer does not change how far the motion is magnified.
    `magnification`, in place of `scale`, states that directly for measured displacement: the
    displacement drawn over the true one, so recordings of different models share one scale.
    """

    geometry: str
    data: str | None = None
    shapes: str | None = None
    mode: int = 1
    width: int = 1920
    height: int = 1080
    fps: float = 30.0
    speed: float | None = None
    seconds: float | None = None
    cycles: float = 3.0
    scale: float = 1.0
    magnification: float | None = None
    opacity: float = 1.0
    view: str = 'iso'
    eye: Sequence[float] | None = None
    up: Sequence[float] | None = None
    frame: Sequence[str] | None = None
    theme: str = 'dark'
    records: Sequence[int] = field(default_factory=tuple)


def animation_view(project: Any, recording: Recording) -> dict[str, Any]:
    """The `shape3d` view the app shows for the recording's selection, opening at its opacity
    and from its camera, or RecordError saying why there is none to play."""
    try:
        view = _shown(project, recording.geometry, recording.data, recording.shapes,
                      recording.records)
        view = staged(view, recording.opacity,
                      opening_camera(recording.view, recording.eye, recording.up),
                      framed(project, recording.geometry, view, recording.frame))
    except RecordError:
        raise
    except AnimationError as error:
        raise RecordError(str(error)) from None
    if view.get('motion') is None and not 1 <= recording.mode <= len(view['modes']):
        raise RecordError(f'{recording.shapes or recording.data} has modes 1 to '
                          f'{len(view["modes"])}; not {recording.mode}')
    return view


def drawn_scale(view: dict[str, Any], recording: Recording) -> float:
    """The amplitude the view is drawn at, as a multiple of the app's own: the recording's
    `scale`, or for a `magnification` the multiple that draws the motion's largest true
    displacement (`motion.peak`, in meters as the nodes are) that many times over."""
    if recording.magnification is None:
        return float(recording.scale)
    magnification = float(recording.magnification)
    if recording.scale != 1.0:
        raise RecordError('give the scale or the magnification, not both')
    if not magnification > 0:
        raise RecordError(f'the magnification is above 0, not {recording.magnification}')
    motion = view.get('motion')
    if motion is None:
        raise RecordError("a magnification is of measured displacement, and a mode's size is "
                          'arbitrary; set the scale instead')
    # a displacement's dimension is length ('modal_length' for modal coordinates)
    if str(motion.get('quantity', '')).lower() not in ('length', 'displacement'):
        raise RecordError(f'a magnification is of displacement, and {recording.data} is '
                          f'{str(motion.get("quantity") or "of no stated quantity").lower()}; '
                          'set the scale instead')
    if view.get('unit') == 'undefined':
        raise RecordError(f'{recording.geometry} has no length unit to magnify against; '
                          'set the scale instead')
    peak = float(motion.get('peak') or 0.0)
    if not peak > 0:
        raise RecordError(f'{recording.data} never moves, so it has no magnification')
    return magnification * peak / amplitude_unit(view)


def record_page(project: Any, recording: Recording) -> str:
    """The report viewer's page with the recording's view in it, ready for a browser to draw."""
    from .report.viewer import DATA, THEMES, viewer_page
    from .service import frames

    if recording.theme not in THEMES:
        raise RecordError(f'the theme is light or dark, not {recording.theme!r}')
    view = animation_view(project, recording)
    setup: dict[str, Any] = {'width': int(recording.width), 'height': int(recording.height),
                             'scale': drawn_scale(view, recording)}
    if view.get('motion') is None:
        setup['mode'] = recording.mode - 1
    frame = frames.pack({'type': 'response', 'id': 0,
                         'result': {'record': {'view': view, 'setup': setup}}})
    data = frame.encode('utf-8') if isinstance(frame, str) else frame
    page = viewer_page()
    if DATA not in page:
        raise RecordError('the report viewer holds no place for data; build it again')
    page = page.replace(DATA, base64.b64encode(data).decode('ascii'), 1)
    return page.replace('<html lang="en">', f'<html lang="en" data-theme="{recording.theme}">', 1)


def frame_times(info: Mapping[str, Any], recording: Recording) -> list[float]:
    """Where each frame is drawn: for measured motion a sample (from 0), for a mode a fraction
    of its cycle. Motion plays at `speed` of real time (once through, at real time, when
    neither it nor `seconds` is given); a mode swings `cycles` cycles of the app's own length."""
    fps = float(recording.fps)
    if not fps > 0:
        raise RecordError('the frame rate is above 0')
    if info.get('motion'):
        samples, step = int(info['samples']), float(info['step'])
        if samples < 1 or not step > 0:
            raise RecordError('the data has no samples to play')
        real = samples * step
        if recording.seconds is not None:
            seconds = float(recording.seconds)
        else:
            speed = 1.0 if recording.speed is None else float(recording.speed)
            if not speed > 0:
                raise RecordError('the speed is above 0')
            seconds = real / speed
        count = max(1, round(seconds * fps))
        rate = real / seconds / step  # samples of data per second of video
        return [min(samples - 1.0, k / fps * rate) for k in range(count)]
    seconds = float(recording.seconds) if recording.seconds is not None else recording.cycles * CYCLE_S
    count = max(1, round(seconds * fps))
    return [(k / fps / CYCLE_S) % 1.0 for k in range(count)]


def find_chrome() -> str:
    """Chrome, Chromium or Edge on this machine, or the one HERTZFORGE_CHROME names."""
    named = os.environ.get('HERTZFORGE_CHROME')
    if named:
        return named
    for path in ('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
                 '/Applications/Chromium.app/Contents/MacOS/Chromium',
                 '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge'):
        if os.path.exists(path):
            return path
    for name in ('google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser',
                 'msedge', 'chrome'):
        found = shutil.which(name)
        if found:
            return found
    # the Chromium Playwright installs (`playwright install chromium`), newest first
    for cache in (Path.home() / 'Library/Caches/ms-playwright', Path.home() / '.cache/ms-playwright'):
        mac = 'chromium-*/chrome-mac*/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'
        for pattern in (mac, 'chromium-*/chrome-linux*/chrome'):
            found = sorted(cache.glob(pattern), reverse=True)
            if found:
                return str(found[0])
    raise RecordError('recording needs Chrome, Chromium or Edge; install one, or name it '
                      'with HERTZFORGE_CHROME')


def find_ffmpeg() -> str:
    """ffmpeg on this machine, or the one HERTZFORGE_FFMPEG names."""
    found = os.environ.get('HERTZFORGE_FFMPEG') or shutil.which('ffmpeg')
    if not found:
        raise RecordError('recording needs ffmpeg; install it, or name it with HERTZFORGE_FFMPEG')
    return found


def chrome_arguments(executable: str, profile: str, width: int, height: int) -> list[str]:
    """The command that starts headless Chrome on a fresh profile at `profile`. It keeps no
    cookies or passwords, so it is told not to use the system's keychain
    (`--use-mock-keychain`, `--password-store=basic`): a browser with no keychain item of its
    own yet, such as a Chromium for testing, otherwise has macOS ask for the login password."""
    return [executable, '--headless=new', '--remote-debugging-port=0',
            f'--user-data-dir={profile}', f'--window-size={width},{height}',
            '--force-device-scale-factor=1', '--hide-scrollbars', '--mute-audio',
            '--no-first-run', '--no-default-browser-check', '--allow-file-access-from-files',
            '--use-mock-keychain', '--password-store=basic', 'about:blank']


class _Chrome:
    """Headless Chrome on one page, driven over its DevTools protocol."""

    def __init__(self, executable: str, width: int, height: int) -> None:
        from websockets.sync.client import connect

        self._profile = tempfile.TemporaryDirectory(prefix='hertzforge-record-')
        self._process = subprocess.Popen(
            chrome_arguments(executable, self._profile.name, width, height),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        port_file = Path(self._profile.name) / 'DevToolsActivePort'
        deadline = time.monotonic() + 30
        while not port_file.exists() or not port_file.read_text().strip():
            if self._process.poll() is not None or time.monotonic() > deadline:
                self.close()
                raise RecordError(f'{executable} did not start headless')
            time.sleep(0.05)
        port = int(port_file.read_text().split()[0])
        with urllib.request.urlopen(f'http://127.0.0.1:{port}/json/list', timeout=10) as answer:
            pages = [t for t in json.load(answer) if t.get('type') == 'page']
        self._socket = connect(pages[0]['webSocketDebuggerUrl'], max_size=None,
                               open_timeout=10)
        self._id = 0
        #: what the page threw or logged as an error, newest last, to say why it did not draw
        self.errors: list[str] = []

    def call(self, method: str, params: Mapping[str, Any] | None = None) -> dict[str, Any]:
        self._id += 1
        self._socket.send(json.dumps({'id': self._id, 'method': method,
                                      'params': dict(params or {})}))
        while True:
            message = json.loads(self._socket.recv())
            self._note(message)
            if message.get('id') == self._id:
                if 'error' in message:
                    raise RecordError(f'Chrome refused {method}: {message["error"]}')
                return message.get('result', {})

    def _note(self, message: Mapping[str, Any]) -> None:
        params = message.get('params', {})
        if message.get('method') == 'Runtime.exceptionThrown':
            details = params.get('exceptionDetails', {})
            self.errors.append(details.get('exception', {}).get('description')
                               or details.get('text', 'an exception'))
        elif message.get('method') == 'Runtime.consoleAPICalled' and params.get('type') == 'error':
            self.errors.append(' '.join(str(a.get('value', a.get('description', '')))
                                        for a in params.get('args', [])))
        elif message.get('method') == 'Inspector.targetCrashed':
            self.errors.append('the page crashed')

    def evaluate(self, expression: str) -> Any:
        result = self.call('Runtime.evaluate', {'expression': expression, 'returnByValue': True})
        if 'exceptionDetails' in result:
            text = result['exceptionDetails'].get('exception', {}).get('description') \
                or result['exceptionDetails'].get('text')
            raise RecordError(f'the page failed: {text}')
        return result.get('result', {}).get('value')

    def close(self) -> None:
        socket = getattr(self, '_socket', None)
        if socket is not None:
            socket.close()
        if self._process.poll() is None:
            self._process.terminate()
            try:
                self._process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self._process.kill()
        self._profile.cleanup()


def record(project: Any, recording: Recording, output: str | os.PathLike,
           progress: Any = None) -> Path:
    """The recording drawn and written to `output`, an MP4; its path. `progress`, when given, is
    called with (frames done, frames in all) as it goes."""
    if recording.width < 16 or recording.height < 16:
        raise RecordError('the video is at least 16 by 16 pixels')
    # H.264 with 4:2:0 color takes even sizes
    recording.width += recording.width % 2
    recording.height += recording.height % 2
    page = record_page(project, recording)
    chrome, ffmpeg = find_chrome(), find_ffmpeg()
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hertzforge-record-page-') as folder:
        path = Path(folder) / 'record.html'
        path.write_text(page, encoding='utf-8')
        browser = _Chrome(chrome, recording.width, recording.height)
        try:
            browser.call('Page.enable')
            browser.call('Runtime.enable')
            browser.call('Inspector.enable')
            browser.call('Page.navigate', {'url': path.as_uri()})
            deadline = time.monotonic() + READY_S
            while not browser.evaluate('!!window.__hertzforgeRecorder'):
                failure = browser.evaluate(
                    "window.__hertzforgeRecorderFailed"
                    " ?? document.querySelector('.failure')?.textContent ?? null")
                if failure or time.monotonic() > deadline:
                    why = failure or (browser.errors[-1] if browser.errors else None)
                    raise RecordError(f'the page did not draw the view in {READY_S:g} s'
                                      + (f': {why}' if why else '; it said nothing'))
                time.sleep(0.1)
            info = browser.evaluate('window.__hertzforgeRecorder.info()')
            times = frame_times(info, recording)
            encoder = subprocess.Popen(
                [ffmpeg, '-y', '-loglevel', 'error', '-f', 'image2pipe', '-c:v', 'png',
                 '-framerate', f'{recording.fps:g}', '-i', '-', '-c:v', 'libx264',
                 '-pix_fmt', 'yuv420p', '-crf', '18', '-movflags', '+faststart', str(output)],
                stdin=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                for k, at in enumerate(times):
                    url = browser.evaluate(f'window.__hertzforgeRecorder.frame({at!r})')
                    encoder.stdin.write(base64.b64decode(url.split(',', 1)[1]))
                    if progress is not None:
                        progress(k + 1, len(times))
                encoder.stdin.close()
                if encoder.wait() != 0:
                    raise RecordError(f'ffmpeg failed: {encoder.stderr.read().decode().strip()}')
            finally:
                if encoder.poll() is None:
                    encoder.kill()
        finally:
            browser.close()
    return output
