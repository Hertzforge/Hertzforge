# hertzforge-backendd

The headless CORE of the split deployment. It runs ON the hardware host and serves a
remote Hertzforge front end over `hertzforge-protocol` v3 on a WebSocket.

**The control loop never crosses the network.** The engine, the watchdog and the drive
clock are all in this process, co-resident with the hardware. What crosses is the
decimated display path down and operator intent up. A remote Stop or Abort is
SUPERVISORY - honored, never the primary abort authority, which is on-hardware and
backed by the physical E-stop.

## Configuration

TOML. Two fields have no default and must be stated:

```toml
# REQUIRED. An EXPLICIT interface. There is deliberately no 0.0.0.0 default: a daemon
# that binds every interface puts a shaker on whatever network the box is plugged
# into, on the day someone runs it without reading this file. A wildcard address is
# REFUSED at start.
bind = "192.168.4.20:8765"

# REQUIRED, non-empty. The shared secret every front end presents at the handshake.
# Not a security system - a shared secret over a plaintext socket on a trusted lab
# network - but the door that stops an accidental second workstation from reaching a
# shaker. A BLANK token admits nobody (and is refused at start, so you find out now
# rather than at the operator's first connect).
token = "change-me"

# Optional, with the defaults shown.

# Captures, recordings and the advisory lockfile. Defaults to $HERTZFORGE_DATA_ROOT, else
# ~/Hertzforge_Data.
# data_root = "/var/lib/hertzforge"

# Seconds between server pings. The client answers with a pong; three missed beats
# and the connection is treated as gone (which is how a CLOSED LAPTOP - no FIN, the
# socket stays "open" for minutes - is detected at all).
heartbeat_s = 5.0

# Seconds a run CONTINUES after the controller stops answering, before the daemon
# brings it down. The run continuing is the point: a network hiccup on the operator's
# laptop must not abort a 40-minute test the hardware host is running fine. When the
# window closes with no reconnect, the stop is the CONTROLLED ramp-down path.
controller_timeout_s = 30.0

# "sim" (the HERTZFORGE-SIM physics plant) or "ni" (NI-DAQmx). NAMED, not discovered: a
# headless daemon has no operator to ask, and "whatever is attached" is exactly the
# substitution the station factory exists to refuse. A delegated controller is
# owned by its module and is not servable headless.
station = "sim"

# The NI device ids the station binds, when station = "ni". EMPTY means every device
# the scan finds, which is a development convenience and not a claim that the first
# device is the right device.
ni_devices = []

# Refuse to start when the NI preflight cannot RESERVE the configured devices.
preflight_required = true
```

Unknown keys are REFUSED, not ignored: a misspelled `token` that parsed and then
defaulted would be the same class of defect as a silently dropped profile field.

## Running it

```sh
# validate the config and run the preflight, then exit
hertzforge-backendd --config /etc/hertzforge/backendd.toml --check

# serve
hertzforge-backendd --config /etc/hertzforge/backendd.toml

# stop the daemon holding this config's data root
hertzforge-backendd --config /etc/hertzforge/backendd.toml --stop

# the soak harness: drive the Recorder through a real socket for N seconds and
# print p99 tick latency + SchedulingFault count against the stated budget
hertzforge-backendd --config /etc/hertzforge/backendd.toml --soak 1800
```

`HERTZFORGE_LOG` overrides the log filter (default `hertzforge=debug,info`).

## The soak gate

`--soak` implements the MEASUREMENT for the soak predicate and runs on the box. It
exits 0 only when BOTH halves pass, and prints every number so the evidence row cites
something checkable rather than a verdict:

- **zero SchedulingFault** (retried or not - `hertzforge-recorder` counts them precisely
  so this predicate is measurable; `run_segment` swallows the retry by
  design, and swallowed-and-uncounted the gate meant nothing);
- **p99 segment latency <= 50% of the budget**, where the budget is the wall time one
  pump segment REPRESENTS (`segment_frames / native_rate_hz`; 250.0 ms at the recorder
  defaults, so the gate is 125.0 ms).

**"NO SAMPLES" is a FAIL, not a blank.** A soak whose pump never ran has proven
nothing.

## Mutual exclusion with the desktop app

The daemon writes an advisory lockfile at `<data_root>/hertzforge-backendd.lock` naming its
pid, host, bind address and station. The Hertzforge desktop app reads it and, when a daemon
holds the NI station, renders `STATION BLOCKED / hardware busy` naming the process and
offering "stop the daemon".

**It never offers a Sim path.** Falling back to the software plant because hardware is
busy would produce data that looks like a measurement and is not.

The lock is ADVISORY: DAQmx's own reservation is the real exclusion and holds whether
or not this file exists. What the file buys is that an unexplainable refusal becomes an
actionable one. A lock whose pid is dead is stale and ignored.

## What it serves

The **Recorder**, the station list, the channel table (daemon-authoritative, with etag
concurrency), the capture LIST verb, and the display streams.

Everything else answers `ProtocolError::LocalOnly`, which a remote UI renders as
"local-only in remote mode". **There is never a fallback**: a front end that quietly
ran a module against its own local backend would put the run on different hardware than
the operator selected.
