#!/usr/bin/env python3
"""Fake MPRIS players and scripted acceptance scenarios on a PRIVATE session bus.

Serves org.mpris.MediaPlayer2.fake1 and org.mpris.MediaPlayer2.fake2, each on
its own connection at /org/mpris/MediaPlayer2 with org.mpris.MediaPlayer2.Player
properties (PlaybackStatus, Metadata, CanGoPrevious/CanGoNext/CanPlay/CanPause),
records every Previous/Next/PlayPause/Play/Pause call as a JSON line, and emits
org.freedesktop.DBus.Properties.PropertiesChanged on property changes.

Selection rule mirrored from the FlameWM MPRIS provider:
  Playing > Paused > Stopped, ties broken by smallest bus name.

Bus safety: the tool refuses to run on the login session bus. If
DBUS_SESSION_BUS_ADDRESS is unset it starts itself under dbus-run-session;
if it is set it must not point at the login bus socket
(unix:path=/run/user/<uid>/bus or $XDG_RUNTIME_DIR/bus). --bus-address applies
the same refusal to the address it is given.

Requires python3-dbus (dbus-python) and python3-gi (GLib main loop); stdlib otherwise.

Usage:
  python3 -I tools/flamewm_mpris_acceptance.py --help
  dbus-run-session -- python3 -I tools/flamewm_mpris_acceptance.py --self-test
  dbus-run-session -- python3 -I tools/flamewm_mpris_acceptance.py --scenario arbitration
  python3 -I tools/flamewm_mpris_acceptance.py --bus-address ADDR --hold 0
  python3 -I tools/flamewm_mpris_acceptance.py --hold 3 --title-a "long title" --statuses Playing,Paused

Exit codes: 0 PASS, 1 FAIL, 3 BLOCKED (environment/bus refusal).
Verdict JSON is printed to stdout. --hold prints one JSON line instead (log path and bus address).
"""

from __future__ import annotations

import argparse
import json
import os
import select
import shutil
import subprocess
import sys
import tempfile
import time

EXIT_PASS, EXIT_FAIL, EXIT_BLOCKED = 0, 1, 3

MPRIS_PREFIX = "org.mpris.MediaPlayer2."
OBJ_PATH = "/org/mpris/MediaPlayer2"
ROOT_IFACE = "org.mpris.MediaPlayer2"
PLAYER_IFACE = "org.mpris.MediaPlayer2.Player"
PROPS_IFACE = "org.freedesktop.DBus.Properties"
FAKE_NAMES = (MPRIS_PREFIX + "fake1", MPRIS_PREFIX + "fake2")
RANK = {"Playing": 0, "Paused": 1, "Stopped": 2}
CAP_PROPS = {"previous": "CanGoPrevious", "next": "CanGoNext", "play": "CanPlay", "pause": "CanPause"}
ALL_CAPS = ["previous", "next", "play", "pause"]
DEFAULT_TITLES = {"fake1": "Song A", "fake2": "Song C"}
DEFAULT_INITIAL = {"fake1": "Playing", "fake2": "Stopped"}

# Each step: (label, action, expected active player short name).
# action: ("set", player, {prop: value}) or ("drop", player).
SCENARIOS = {
    "arbitration": {
        "initial": {"fake1": "Stopped", "fake2": "Stopped"},
        "steps": [
            ("fake1 Paused", ("set", "fake1", {"PlaybackStatus": "Paused"}), "fake1"),
            ("fake2 Playing", ("set", "fake2", {"PlaybackStatus": "Playing"}), "fake2"),
            ("fake1 Playing (tie, smallest name wins)", ("set", "fake1", {"PlaybackStatus": "Playing"}), "fake1"),
            ("fake2 Stopped", ("set", "fake2", {"PlaybackStatus": "Stopped"}), "fake1"),
        ],
    },
    "disappear": {
        "initial": {"fake1": "Paused", "fake2": "Playing"},
        "steps": [("fake2 drops its bus name", ("drop", "fake2"), "fake1")],
    },
    "capabilities": {
        "initial": {"fake1": "Playing", "fake2": "Stopped"},
        "steps": [
            ("fake1 CanPause=false", ("set", "fake1", {"CanPause": False}), "fake1"),
            ("fake1 CanGoNext=false", ("set", "fake1", {"CanGoNext": False}), "fake1"),
            ("fake1 CanPause=true", ("set", "fake1", {"CanPause": True}), "fake1"),
        ],
    },
    "title-change": {
        "initial": {"fake1": "Playing", "fake2": "Stopped"},
        "steps": [("fake1 title becomes Song B", ("set", "fake1", {"Metadata": "Song B"}), "fake1")],
    },
}


def blocked(reason: str, check: str) -> int:
    emit({"tool": "flamewm_mpris_acceptance", "check": check, "status": "BLOCKED", "reason": reason})
    return EXIT_BLOCKED


def emit(obj: dict) -> None:
    print(json.dumps(obj, indent=2, sort_keys=True))


def parse_caps(text: str) -> list:
    """Comma list of capabilities that start TRUE (previous,next,play,pause). Empty string clears all."""
    names = [t.strip() for t in text.split(",")] if text.strip() else []
    bad = [n for n in names if n not in CAP_PROPS]
    if bad:
        raise argparse.ArgumentTypeError(f"unknown capability {bad[0]!r}; choose from previous,next,play,pause")
    return names


def parse_statuses(text: str) -> list:
    parts = [t.strip() for t in text.split(",")]
    if len(parts) != 2 or any(p not in RANK for p in parts):
        raise argparse.ArgumentTypeError("expected A,B where each is Playing, Paused or Stopped")
    return parts


def build_setup(scenario, opts) -> dict:
    """Initial state for fake1/fake2. opts keys: title_a, title_b, caps, statuses (all optional)."""
    opts = opts or {}
    statuses = dict(SCENARIOS[scenario]["initial"]) if scenario in SCENARIOS else dict(DEFAULT_INITIAL)
    if opts.get("statuses") is not None:
        statuses = {"fake1": opts["statuses"][0], "fake2": opts["statuses"][1]}
    titles = {
        "fake1": opts["title_a"] if opts.get("title_a") is not None else DEFAULT_TITLES["fake1"],
        "fake2": opts["title_b"] if opts.get("title_b") is not None else DEFAULT_TITLES["fake2"],
    }
    caps = opts["caps"] if opts.get("caps") is not None else list(ALL_CAPS)
    return {"titles": titles, "statuses": statuses, "caps": caps}


def select_active(statuses: dict) -> str | None:
    """Playing > Paused > Stopped, ties by smallest bus name. statuses: bus name -> PlaybackStatus."""
    cands = [(RANK[s], name) for name, s in statuses.items() if s in RANK]
    return min(cands)[1] if cands else None


def login_bus_candidates() -> set:
    uid = os.getuid()
    cands = {f"unix:path=/run/user/{uid}/bus"}
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        cands.add(f"unix:path={runtime}/bus")
    return cands


def check_bus_safety(address: str) -> str | None:
    """Return a refusal reason if the address is (or looks like) the login session bus."""
    for alt in address.split(";"):
        if alt.split(",")[0] in login_bus_candidates():
            return f"bus address {address} points at the login session bus; refusing"
    return None


def import_dbus():
    import dbus  # noqa: F401
    import dbus.mainloop.glib  # noqa: F401
    import dbus.exceptions  # noqa: F401
    import dbus.service  # noqa: F401
    from gi.repository import GLib  # noqa: F401

    return dbus, GLib


def box(key: str, value):
    import dbus

    if key == "Metadata":
        return dbus.Dictionary(
            {
                "mpris:trackid": dbus.ObjectPath("/org/mpris/MediaPlayer2/Track/1"),
                "xesam:title": dbus.String(value),
                "xesam:artist": dbus.Array(["Fake Artist"], signature="s"),
            },
            signature="sv",
        )
    if isinstance(value, bool):
        return dbus.Boolean(value)
    return dbus.String(value)


def make_player_class(dbus):
    class Player(dbus.service.Object):
        def __init__(self, conn, logpath, props):
            self.logpath = logpath
            caps = props["caps"]
            self.props = {
                "PlaybackStatus": box("PlaybackStatus", props["status"]),
                "Metadata": box("Metadata", props["title"]),
                "CanGoPrevious": box("CanGoPrevious", "previous" in caps),
                "CanGoNext": box("CanGoNext", "next" in caps),
                "CanPlay": box("CanPlay", "play" in caps),
                "CanPause": box("CanPause", "pause" in caps),
                "Position": dbus.Int64(0),
                "Volume": dbus.Double(1.0),
                "Rate": dbus.Double(1.0),
                "MinimumRate": dbus.Double(1.0),
                "MaximumRate": dbus.Double(1.0),
                "CanSeek": dbus.Boolean(False),
                "CanControl": dbus.Boolean(True),
            }
            self.root_props = {
                "Identity": dbus.String(props["identity"]),
                "CanQuit": dbus.Boolean(False),
                "CanRaise": dbus.Boolean(False),
                "HasTrackList": dbus.Boolean(False),
                "SupportedUriSchemes": dbus.Array([], signature="s"),
                "SupportedMimeTypes": dbus.Array([], signature="s"),
            }
            super().__init__(conn, OBJ_PATH)

        def _props_for(self, interface):
            tables = {ROOT_IFACE: self.root_props, PLAYER_IFACE: self.props}
            if str(interface) not in tables:
                raise dbus.exceptions.DBusException(
                    f"unknown interface {interface!r}", name="org.freedesktop.DBus.Error.InvalidArgs")
            return tables[str(interface)]

        @dbus.service.method(ROOT_IFACE, in_signature="", out_signature="")
        def Raise(self):
            pass

        @dbus.service.method(ROOT_IFACE, in_signature="", out_signature="")
        def Quit(self):
            pass

        def _record(self, method: str) -> None:
            append_log(self.logpath, {"event": "call", "method": method, "player": self.name_tag, "t": time.time()})

        @dbus.service.method(PLAYER_IFACE, in_signature="", out_signature="")
        def Next(self):
            self._record("Next")

        @dbus.service.method(PLAYER_IFACE, in_signature="", out_signature="")
        def Previous(self):
            self._record("Previous")

        @dbus.service.method(PLAYER_IFACE, in_signature="", out_signature="")
        def PlayPause(self):
            self._record("PlayPause")

        @dbus.service.method(PLAYER_IFACE, in_signature="", out_signature="")
        def Play(self):
            self._record("Play")

        @dbus.service.method(PLAYER_IFACE, in_signature="", out_signature="")
        def Pause(self):
            self._record("Pause")

        @dbus.service.method(PROPS_IFACE, in_signature="ss", out_signature="v")
        def Get(self, interface, prop):
            return self._props_for(interface)[str(prop)]

        @dbus.service.method(PROPS_IFACE, in_signature="s", out_signature="a{sv}")
        def GetAll(self, interface):
            return dbus.Dictionary(self._props_for(interface), signature="sv")

        @dbus.service.signal(PROPS_IFACE, signature="sa{sv}as")
        def PropertiesChanged(self, interface, changed, invalidated):
            pass

        def set_props(self, changes: dict) -> None:
            self.props.update(changes)
            self.PropertiesChanged(PLAYER_IFACE, dbus.Dictionary(changes, signature="sv"), [])

    return Player


def append_log(path: str, obj: dict) -> None:
    with open(path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(obj, sort_keys=True) + "\n")


def export_players(dbus, logpath: str, setup: dict):
    """Register fake1 and fake2 on the session bus named by DBUS_SESSION_BUS_ADDRESS.
    Returns (conns, players, statuses); the caller must keep them referenced."""
    Player = make_player_class(dbus)
    conns, players, statuses = {}, {}, {}
    for short in ("fake1", "fake2"):
        name = MPRIS_PREFIX + short
        conn = dbus.SessionBus(private=True)
        conns[short] = conn
        status = setup["statuses"][short]
        identity = "Fake Player " + short.removeprefix("fake")
        obj = Player(conn, logpath, {"status": status, "title": setup["titles"][short], "caps": setup["caps"],
                                     "identity": identity})
        obj.name_tag = name
        players[short] = obj
        statuses[name] = status
        # keep the BusName reference alive for the life of the process
        conn._flamewm_busname = dbus.service.BusName(name, bus=conn)
    return conns, players, statuses


def serve(args) -> int:
    """Child mode: export fake players, wait for GO on stdin, run scenario steps."""
    dbus, GLib = import_dbus()
    from dbus.mainloop.glib import DBusGMainLoop

    DBusGMainLoop(set_as_default=True)
    scenario = SCENARIOS[args.scenario]
    setup = json.loads(args.setup_json) if args.setup_json else build_setup(args.scenario, None)
    conns, players, statuses = export_players(dbus, args.log, setup)
    print("READY", flush=True)
    if sys.stdin.readline().strip() != "GO":
        return EXIT_FAIL

    loop = GLib.MainLoop()
    timeline = list(scenario["steps"])

    def run_step(index: int) -> bool:
        if index >= len(timeline):
            GLib.timeout_add(args.hold_ms, finish)
            return False
        label, action, expected = timeline[index]
        if action[0] == "set":
            _, short, changes = action
            boxed = {k: box(k, v) for k, v in changes.items()}
            players[short].set_props(boxed)
            if "PlaybackStatus" in changes:
                statuses[MPRIS_PREFIX + short] = changes["PlaybackStatus"]
        elif action[0] == "drop":
            _, short = action
            # release_name, not Bus.close(): closing a dbus-python connection under GLib exits the process
            conns[short].release_name(MPRIS_PREFIX + short)
            statuses.pop(MPRIS_PREFIX + short, None)
        computed = select_active(statuses)
        append_log(args.log, {
            "event": "step", "index": index, "label": label,
            "expected_active": MPRIS_PREFIX + expected, "computed_active": computed,
        })
        GLib.timeout_add(args.step_ms, run_step, index + 1)
        return False

    def finish() -> bool:
        print("DONE", flush=True)
        loop.quit()
        return False

    GLib.timeout_add(args.step_ms, run_step, 0)
    loop.run()
    return EXIT_PASS


def hold_mode(seconds: float, setup: dict) -> int:
    """Register the fake players and keep serving (logging calls) until terminated, or for SECONDS if > 0."""
    dbus, GLib = import_dbus()
    from dbus.mainloop.glib import DBusGMainLoop

    DBusGMainLoop(set_as_default=True)
    tmp = tempfile.mkdtemp(prefix="flamewm-mpris-hold-")
    logpath = os.path.join(tmp, "log.jsonl")
    open(logpath, "w").close()
    _conns, _players, statuses = export_players(dbus, logpath, setup)
    print(json.dumps({
        "tool": "flamewm_mpris_acceptance", "check": "hold", "status": "HOLDING",
        "log": logpath, "bus_address": os.environ.get("DBUS_SESSION_BUS_ADDRESS", ""),
        "names": list(FAKE_NAMES), "statuses": statuses, "titles": setup["titles"], "caps": setup["caps"],
    }, sort_keys=True), flush=True)
    loop = GLib.MainLoop()

    def stop() -> bool:
        loop.quit()
        return False

    if seconds > 0:
        GLib.timeout_add(int(seconds * 1000), stop)
    loop.run()
    return EXIT_PASS


def run_session(scenario: str, step_ms: int, hold_ms: int, extra=None, setup=None) -> dict:
    dbus, GLib = import_dbus()
    from dbus.mainloop.glib import DBusGMainLoop

    if setup is None:
        setup = build_setup(scenario, None)
    tmp = tempfile.mkdtemp(prefix="flamewm-mpris-")
    logpath = os.path.join(tmp, "log.jsonl")
    open(logpath, "w").close()
    cmd = [sys.executable, "-I", os.path.abspath(__file__), "--serve", "--scenario", scenario,
           "--log", logpath, "--step-ms", str(step_ms), "--hold-ms", str(hold_ms),
           "--setup-json", json.dumps(setup)]
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True)
    ready, _, _ = select.select([proc.stdout], [], [], 15)
    line = proc.stdout.readline().strip() if ready else ""
    if line != "READY":
        proc.kill()
        err = proc.communicate()[1]
        return {"error": "server not ready", "stderr": err[-2000:]}

    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus(mainloop=None)
    observed = {"PropertiesChanged": [], "NameOwnerChanged": []}

    def on_props(interface, changed, invalidated, path=None):
        if str(interface) != PLAYER_IFACE:
            return
        entry = {"kind": "PropertiesChanged", "path": str(path), "keys": sorted(str(k) for k in changed)}
        if "Metadata" in changed:
            entry["title"] = str(changed["Metadata"].get("xesam:title", ""))
        if "PlaybackStatus" in changed:
            entry["PlaybackStatus"] = str(changed["PlaybackStatus"])
        if "CanPause" in changed:
            entry["CanPause"] = bool(changed["CanPause"])
        if "CanGoNext" in changed:
            entry["CanGoNext"] = bool(changed["CanGoNext"])
        observed["PropertiesChanged"].append(entry)

    def on_noc(name, old, new):
        if str(name).startswith(MPRIS_PREFIX):
            observed["NameOwnerChanged"].append({"kind": "NameOwnerChanged", "name": str(name), "new_owner": str(new)})

    bus.add_signal_receiver(on_props, signal_name="PropertiesChanged", dbus_interface=PROPS_IFACE,
                            path_keyword="path")
    bus.add_signal_receiver(on_noc, signal_name="NameOwnerChanged", dbus_interface="org.freedesktop.DBus",
                            bus_name="org.freedesktop.DBus", path="/org/freedesktop/DBus")

    proc.stdin.write("GO\n")
    proc.stdin.flush()
    extra_result = extra(bus, logpath) if extra else {}

    loop = GLib.MainLoop()
    deadline = time.time() + 30

    def poll():
        if proc.poll() is not None or time.time() > deadline:
            GLib.timeout_add(200, loop.quit)
            return False
        return True

    GLib.timeout_add(50, poll)
    loop.run()
    out = proc.stdout.read()
    err = proc.stderr.read()
    if proc.poll() is None:
        proc.kill()
    with open(logpath, encoding="utf-8") as fh:
        events = [json.loads(x) for x in fh if x.strip()]
    return {
        "rc": proc.returncode,
        "stdout": out.strip().splitlines(),
        "stderr": err[-2000:],
        "events": events,
        "observed": observed,
        "extra": extra_result,
        "logpath": logpath,
    }


def scenario_checks(name: str, result: dict) -> list:
    steps = SCENARIOS[name]["steps"]
    events = result["events"]
    step_events = [e for e in events if e.get("event") == "step"]
    obs = result["observed"]
    checks = [
        {"name": "server_exit_0", "ok": result["rc"] == 0},
        {"name": "scenario_done", "ok": "DONE" in result["stdout"]},
        {"name": "all_steps_logged", "ok": len(step_events) == len(steps)},
        {"name": "arbitration_table_matches_rule",
         "ok": all(e["expected_active"] == e["computed_active"] for e in step_events),
         "detail": [(e["label"], e["expected_active"], e["computed_active"]) for e in step_events]},
    ]
    if name == "arbitration":
        n = sum(1 for p in obs["PropertiesChanged"] if "PlaybackStatus" in p.get("keys", []))
        checks.append({"name": "playback_status_changes_observed", "ok": n >= len(steps), "detail": n})
    elif name == "disappear":
        gone = [o for o in obs["NameOwnerChanged"] if o["name"] == MPRIS_PREFIX + "fake2" and o["new_owner"] == ""]
        checks.append({"name": "fake2_name_owner_lost_observed", "ok": bool(gone)})
    elif name == "capabilities":
        keys = {k for p in obs["PropertiesChanged"] for k in p.get("keys", [])}
        checks.append({"name": "capability_changes_observed",
                       "ok": {"CanPause", "CanGoNext"} <= keys, "detail": sorted(keys)})
    elif name == "title-change":
        titles = [p.get("title") for p in obs["PropertiesChanged"]]
        checks.append({"name": "title_change_observed", "ok": "Song B" in titles, "detail": titles})
    return checks


def run_scenario(name: str, step_ms: int, hold_ms: int, opts=None) -> int:
    result = run_session(name, step_ms, hold_ms, setup=build_setup(name, opts))
    if "error" in result:
        return blocked(result["error"] + ": " + result["stderr"], name)
    checks = scenario_checks(name, result)
    ok = all(c["ok"] for c in checks)
    emit({"tool": "flamewm_mpris_acceptance", "check": name, "status": "PASS" if ok else "FAIL",
          "checks": checks, "observed": result["observed"]})
    return EXIT_PASS if ok else EXIT_FAIL


def self_test(step_ms: int, hold_ms: int) -> int:
    dbus, _ = import_dbus()
    failures = []

    def selftest_extra(bus, logpath):
        out = {}
        names = set(str(n) for n in bus.list_names())
        out["both_names_owned"] = all(n in names for n in FAKE_NAMES)
        obj = bus.get_object(FAKE_NAMES[0], OBJ_PATH)
        dbus.Interface(obj, PLAYER_IFACE).PlayPause()
        out["playpause_sent"] = True
        out["playback_status"] = str(dbus.Interface(obj, PROPS_IFACE).Get(PLAYER_IFACE, "PlaybackStatus"))
        identities = {}
        for name in FAKE_NAMES:
            root = bus.get_object(name, OBJ_PATH)
            identities[name] = str(dbus.Interface(root, PROPS_IFACE).GetAll(ROOT_IFACE).get("Identity", ""))
        out["root_identities"] = identities
        return out

    result = run_session("title-change", step_ms, hold_ms, extra=selftest_extra)
    if "error" in result:
        return blocked(result["error"] + ": " + result["stderr"], "self-test")
    checks = [
        {"name": "private_bus_both_players_listed", "ok": result["extra"].get("both_names_owned") is True},
        {"name": "playpause_call_sent", "ok": result["extra"].get("playpause_sent") is True},
        {"name": "playpause_logged_for_fake1",
         "ok": any(e.get("event") == "call" and e.get("method") == "PlayPause"
                   and e.get("player") == FAKE_NAMES[0] for e in result["events"])},
        {"name": "get_playback_status_roundtrip", "ok": result["extra"].get("playback_status") == "Playing"},
        {"name": "root_getall_identity_both_players",
         "ok": result["extra"].get("root_identities") == {FAKE_NAMES[0]: "Fake Player 1",
                                                          FAKE_NAMES[1]: "Fake Player 2"},
         "detail": result["extra"].get("root_identities")},
        {"name": "properties_changed_observed",
         "ok": any("Song B" == p.get("title") for p in result["observed"]["PropertiesChanged"])},
        {"name": "tie_breaks_by_smallest_bus_name",
         "ok": select_active({FAKE_NAMES[1]: "Playing", FAKE_NAMES[0]: "Playing"}) == FAKE_NAMES[0]},
        {"name": "playing_beats_paused",
         "ok": select_active({FAKE_NAMES[0]: "Paused", FAKE_NAMES[1]: "Playing"}) == FAKE_NAMES[1]},
    ]
    checks += scenario_checks("title-change", result)
    failures = [c["name"] for c in checks if not c["ok"]]
    emit({"tool": "flamewm_mpris_acceptance", "check": "self-test",
          "status": "PASS" if not failures else "FAIL", "failures": failures, "checks": checks})
    return EXIT_PASS if not failures else EXIT_FAIL


def preflight(argv: list, check: str):
    """Return an exit code if the run must stop before doing anything, else None."""
    try:
        import_dbus()
    except ImportError as exc:
        return blocked(f"missing python module {exc.name!r}; need python3-dbus and python3-gi", check)
    address = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    if not address:
        if not shutil.which("dbus-run-session"):
            return blocked("no DBUS_SESSION_BUS_ADDRESS and dbus-run-session is not installed", check)
        cmd = ["dbus-run-session", "--", sys.executable, "-I", os.path.abspath(__file__)] + argv
        return subprocess.call(cmd)
    refusal = check_bus_safety(address)
    if refusal:
        return blocked(refusal + "; run under: dbus-run-session -- python3 -I tools/flamewm_mpris_acceptance.py ...", check)
    return None


ATTACH_HELP = (
    "Attach to the xephyr private bus: scripts/xephyr re-execs itself under dbus-run-session "
    "(FLAMEWM_XEPHYR_PRIVATE_DBUS_INNER=1); the inner session's DBUS_SESSION_BUS_ADDRESS is in the "
    "environment of the harness processes it starts. From such a shell, pass it explicitly: "
    "--bus-address \"$DBUS_SESSION_BUS_ADDRESS\". The login session bus is always refused."
)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                     epilog="Run under dbus-run-session or let the tool start a private bus. " + ATTACH_HELP)
    parser.add_argument("--self-test", action="store_true", help="start two fake players and verify calls and signals")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), help="run a scripted scenario")
    parser.add_argument("--step-ms", type=int, default=300, help="delay between scenario steps (default 300)")
    parser.add_argument("--hold-ms", type=int, default=500, help="hold time after the last step (default 500)")
    parser.add_argument("--title-a", metavar="TEXT", help="fake1 Metadata xesam:title (default 'Song A')")
    parser.add_argument("--title-b", metavar="TEXT", help="fake2 Metadata xesam:title (default 'Song C')")
    parser.add_argument("--initial-caps", type=parse_caps, metavar="LIST",
                        help="comma list of capabilities that start true, from previous,next,play,pause; "
                             "'' clears all (default all true); applies to both players")
    parser.add_argument("--statuses", type=parse_statuses, metavar="A,B",
                        help="initial PlaybackStatus of fake1,fake2 (Playing, Paused or Stopped; "
                             "default: scenario initial, or Playing,Stopped without a scenario)")
    parser.add_argument("--hold", type=float, metavar="SECONDS",
                        help="register the fake players and keep logging calls to the JSON log; "
                             "prints one JSON line with log path and bus address; 0 = until terminated")
    parser.add_argument("--bus-address", metavar="ADDR",
                        help="connect the fake players to this existing private session bus instead of starting "
                             "one; refused if it is the login bus. " + ATTACH_HELP)
    parser.add_argument("--serve", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--log", help=argparse.SUPPRESS)
    parser.add_argument("--setup-json", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    raw = sys.argv[1:] if argv is None else list(argv)

    if not (args.self_test or args.scenario or args.serve or args.hold is not None):
        parser.print_help()
        return EXIT_BLOCKED
    if sum(bool(x) for x in (args.self_test, args.scenario, args.hold is not None)) > 1:
        parser.error("--self-test, --scenario and --hold are mutually exclusive")
    check = ("self-test" if args.self_test else (args.scenario or ("hold" if args.hold is not None else "serve")))

    if args.serve:
        if not args.log or not args.scenario:
            return blocked("--serve requires --scenario and --log", check)
        return serve(args)

    opts = {"title_a": args.title_a, "title_b": args.title_b, "caps": args.initial_caps,
            "statuses": args.statuses}
    if args.bus_address is not None:
        if not args.bus_address:
            return blocked("--bus-address is empty", check)
        refusal = check_bus_safety(args.bus_address)
        if refusal:
            return blocked(refusal + "; " + ATTACH_HELP, check)
        os.environ["DBUS_SESSION_BUS_ADDRESS"] = args.bus_address

    stop = preflight(raw, check)
    if stop is not None:
        return stop
    if args.self_test:
        return self_test(args.step_ms, args.hold_ms)
    if args.hold is not None:
        return hold_mode(args.hold, build_setup(None, opts))
    return run_scenario(args.scenario, args.step_ms, args.hold_ms, opts)


if __name__ == "__main__":
    sys.exit(main())
