#!/usr/bin/env python3
"""Acceptance checks for the FlameWM Debian package.

Usage: python3 -I tools/flamewm_deb_acceptance.py [--deb PATH] [--work-dir DIR]

Prints a JSON report to stdout. Exit codes: 0 no FAIL, 1 at least one FAIL,
2 usage error, 3 environment blocked (no deb or required tool missing).
"""

import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def workspace_version() -> str:
    with (REPO_ROOT / "Cargo.toml").open("rb") as fh:
        return tomllib.load(fh)["workspace"]["package"]["version"]
BINARIES = [
    "flamewm",
    "flamewm-session",
    "flamewm-desktop",
    "flamewm-shell",
    "flamewm-settings",
    "flamewm-render-x11",
    "flamewm-render-compile",
    "flamewm-render-inspect",
    "flamewm-lock",
    "flamewm-wayland",
]
REQUIRED_DEPENDS = ["libx11-6", "libxi6", "libxft2", "libfontconfig1", "libdbus-1-3", "libpulse0",
                    "libpam0g", "libpam-runtime", "python3"]
FORBIDDEN_RECOMMENDS = {"ffmpeg", "imagemagick", "poppler-utils", "pandoc", "x11-xserver-utils",
                        "xdg-desktop-portal-kde", "dbus-user-session", "kwallet6", "libpam-kwallet5",
                        "dex", "lxqt-policykit", "xfce4-notifyd", "x11-utils"}
CONFFILES = "/etc/pam.d/flamewm-lock\n"
LEAK_MARKERS = ["build-root", "/run/media", "/home/", "/SDD/"]
PAM_PATH = "etc/pam.d/flamewm-lock"
XSESSION_PATH = "usr/share/xsessions/flamewm.desktop"

PROBE_TIMEOUT = 5
CONTENTS_RE = re.compile(r"^(\S+)\s+(\S+)\s+\d+\s+\S+\s+\S+\s+(.*)$")


def check(cid, status, detail):
    return {"id": cid, "status": status, "detail": detail}


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


def parse_info(text):
    fields = {}
    for line in text.splitlines():
        m = re.match(r"^\s*([A-Za-z][\w-]*):\s*(.*)$", line)
        if m and m.group(1) not in fields:
            fields[m.group(1)] = m.group(2).strip()
    return fields


def parse_contents(text):
    """Return list of (mode, owner, path) with leading './' stripped."""
    entries = []
    for line in text.splitlines():
        m = CONTENTS_RE.match(line)
        if not m:
            continue
        path = m.group(3).split(" -> ")[0]
        if path.startswith("./"):
            path = path[2:]
        entries.append((m.group(1), m.group(2), path.rstrip("/")))
    return entries


def check_binary_modes(entries):
    """Return list of problems for the required binaries (empty when ok)."""
    problems = []
    by_path = {p: (mode, owner) for mode, owner, p in entries}
    for name in BINARIES:
        rel = f"usr/bin/{name}"
        if rel not in by_path:
            problems.append(f"{rel} missing")
            continue
        mode, owner = by_path[rel]
        if mode != "-rwxr-xr-x" or owner != "root/root":
            problems.append(f"{rel} mode={mode} owner={owner}")
    return problems


def check_conffiles(text):
    """Return problems with DEBIAN/conffiles (empty when it lists exactly the PAM file)."""
    if text == CONFFILES:
        return []
    return [f"conffiles={text!r} expected={CONFFILES!r}"]


# The bundled Xcursor pack remains mandatory. PlagueSur/Fluent icon packs are
# installable choices; their upstream archives are not shipped without license evidence.
ICON_THEMES = ["FlameWM-Breeze-Dark"]
ALLOWED_ICON_THEMES = frozenset((*ICON_THEMES, "McMojave-cursors", "PlagueSur", "Fluent"))
LICENSES_PREFIX = "usr/share/doc/flamewm/LICENSES/"
COPYRIGHT_PATH = "usr/share/doc/flamewm/copyright"


def check_shipped_assets(entries):
    """Return checks for icon themes, doc/LICENSES and copyright from --contents entries."""
    paths = [p for _, _, p in entries]
    checks = []
    for theme in ICON_THEMES:
        prefix = f"usr/share/icons/{theme}/"
        present = any(p.startswith(prefix) and p != prefix.rstrip("/") for p in paths)
        checks.append(check(f"icon_theme_{theme}", "PASS" if present else "FAIL",
                            f"{prefix} has files" if present else f"{prefix} missing or empty"))
    unexpected = sorted({
        path.split("/")[3] for path in paths
        if path.startswith("usr/share/icons/") and len(path.split("/")) > 3
        and path.split("/")[3] not in ALLOWED_ICON_THEMES
    })
    checks.append(check("no_removed_icon_themes", "FAIL" if unexpected else "PASS",
                        "unexpected installed themes: " + ", ".join(unexpected) if unexpected
                        else "only selected icon themes and the cursor pack are shipped"))
    licenses = [p for p in paths if p.startswith(LICENSES_PREFIX) and p != LICENSES_PREFIX.rstrip("/")]
    checks.append(check("doc_licenses_present", "PASS" if licenses else "FAIL",
                        f"{len(licenses)} files under {LICENSES_PREFIX}" if licenses
                        else f"{LICENSES_PREFIX} missing or empty"))
    has_copyright = COPYRIGHT_PATH in paths
    checks.append(check("doc_copyright_present", "PASS" if has_copyright else "FAIL", COPYRIGHT_PATH))
    return checks


def find_leaks(paths):
    return [p for p in paths if any(mk in p for mk in LEAK_MARKERS)]


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def parse_sha256sums(text):
    table = {}
    for line in text.splitlines():
        parts = line.split(None, 1)
        if len(parts) == 2:
            table[os.path.basename(parts[1].strip().lstrip("*"))] = parts[0].lower()
    return table


def depend_names(depends):
    names = []
    for alt in depends.split(","):
        first = alt.split("|")[0].strip()
        name = re.split(r"[\s(]", first, maxsplit=1)[0]
        if name:
            names.append(name)
    return names


def probe_env():
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LANG": "C"}
    return env


def probe_binary(path):
    """Return (status, detail) using non-interactive flags; never PASS on unknown flags."""
    for flag in ("--version", "--help"):
        try:
            r = run([path, flag], timeout=PROBE_TIMEOUT, stdin=subprocess.DEVNULL,
                    env=probe_env(), cwd="/")
        except subprocess.TimeoutExpired:
            continue
        except OSError as exc:
            return "FAIL", f"{flag} could not execute: {exc}"
        if r.returncode == 0 and (r.stdout + r.stderr).strip():
            return "PASS", f"{flag} exit 0"
    return "NOT_RUN", "no non-interactive --version/--help exited 0 (flag unknown or not supported)"


def check_ldd(path):
    try:
        r = run(["ldd", path], timeout=PROBE_TIMEOUT, stdin=subprocess.DEVNULL, env=probe_env())
    except (OSError, subprocess.TimeoutExpired) as exc:
        return "NOT_RUN", f"ldd unavailable: {exc}"
    text = r.stdout + r.stderr
    if "not found" in text or r.returncode != 0 and "not a dynamic" not in text:
        return "FAIL", "unresolved shared library: " + " ".join(
            line.strip() for line in text.splitlines() if "not found" in line or "error" in line.lower())
    return "PASS", "all shared libraries resolve"


def dpkg_base(admindir, root):
    return ["dpkg", f"--root={root}", f"--admindir={admindir}", f"--log={root.parent / 'dpkg.log'}",
            "--force-not-root", "--force-bad-path", "--force-depends", "--no-triggers"]


def native_debian_architecture():
    """Return dpkg's installable host architecture; never return source-only 'any'."""
    try:
        result = run(["dpkg", "--print-architecture"], timeout=PROBE_TIMEOUT)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def architecture_issues(declared, filename, native_arch):
    """Enforce a real binary architecture, correct suffix and host compatibility."""
    problems = []
    if not declared or declared in ("any", "all", "source") or not re.fullmatch(r"[a-z0-9][a-z0-9-]*", declared):
        problems.append(f"invalid binary Architecture={declared!r}")
    if declared and not filename.endswith(f"_{declared}.deb"):
        problems.append(f"filename {filename!r} mismatches Architecture={declared!r}")
    if not native_arch:
        problems.append("could not determine host dpkg architecture")
    elif declared != native_arch:
        problems.append(f"target {declared!r} cannot be installed on host {native_arch!r}")
    return problems


def dpkg_lifecycle(deb, work):
    root = work / "root"
    admindir = root / "var/lib/dpkg"
    (admindir / "info").mkdir(parents=True, exist_ok=True)
    (admindir / "updates").mkdir(parents=True, exist_ok=True)
    (admindir / "status").touch()
    base = dpkg_base(admindir, root)
    results = []
    steps = [
        ("dpkg_fresh_install", ["-i", deb]),
        ("dpkg_reinstall_same_version", ["-i", deb]),
    ]
    for cid, argv in steps:
        r = run(base + argv, env=probe_env())
        if r.returncode != 0:
            detail = (r.stderr or r.stdout).strip()[-400:]
            results.append(check(cid, "FAIL", f"exit {r.returncode}: {detail}"))
            return results
        results.append(check(cid, "PASS", "exit 0"))
    st = run(["dpkg", f"--admindir={admindir}", "-s", "flamewm"], env=probe_env())
    installed = st.returncode == 0 and "install ok installed" in st.stdout
    results.append(check("dpkg_status_installed", "PASS" if installed else "FAIL",
                         "install ok installed" if installed else f"exit {st.returncode}"))
    r = run(base + ["-r", "flamewm"], env=probe_env())
    if r.returncode != 0:
        results.append(check("dpkg_remove", "FAIL", f"exit {r.returncode}: {r.stderr.strip()[-400:]}"))
        return results
    leftover = [n for n in BINARIES if (root / "usr/bin" / n).exists()]
    results.append(check("dpkg_remove", "PASS" if not leftover else "FAIL",
                         "exit 0, binaries removed" if not leftover else f"left behind: {leftover}"))
    return results


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--deb", help="deb to test (default: newest .target/dist/*.deb)")
    ap.add_argument("--work-dir", help="new or empty directory for extraction and dpkg root")
    args = ap.parse_args(argv)

    checks = []
    blocked = []
    for tool in ("dpkg-deb", "dpkg"):
        if shutil.which(tool) is None:
            blocked.append(f"{tool} not found")

    if args.deb:
        deb = Path(args.deb).resolve()
    else:
        candidates = glob.glob(str(REPO_ROOT / ".target/dist/*.deb"))
        deb = Path(max(candidates, key=os.path.getmtime)).resolve() if candidates else None
    if deb is None or not deb.is_file():
        blocked.append(f"deb not found: {args.deb or '.target/dist/*.deb'}")

    work = None
    if args.work_dir:
        work = Path(args.work_dir).resolve()
        if work.exists() and (not work.is_dir() or any(work.iterdir())):
            print(f"--work-dir must be new or empty: {work}", file=sys.stderr)
            return 2
    else:
        work = Path(__import__("tempfile").mkdtemp(prefix="flamewm-deb-"))

    if blocked:
        print(json.dumps({"blocked": blocked}, indent=2))
        return 3

    work.mkdir(parents=True, exist_ok=True)
    report = {"deb": str(deb), "work_dir": str(work), "checks": checks}

    # 1. Metadata.
    info = run(["dpkg-deb", "--info", str(deb)])
    fields = parse_info(info.stdout) if info.returncode == 0 else {}
    expected_version = workspace_version()
    if info.returncode != 0:
        checks.append(check("deb_info", "FAIL", f"dpkg-deb --info exit {info.returncode}"))
    else:
        checks.append(check("deb_info", "PASS", "dpkg-deb --info exit 0"))
        checks.append(check("deb_package", "PASS" if fields.get("Package") == "flamewm" else "FAIL",
                            f"Package={fields.get('Package')}"))
        ver_ok = expected_version is not None and fields.get("Version") == expected_version
        checks.append(check("deb_version", "PASS" if ver_ok else ("NOT_RUN" if expected_version is None else "FAIL"),
                            f"Version={fields.get('Version')} workspace={expected_version}"))
        issues = architecture_issues(fields.get("Architecture"), deb.name, native_debian_architecture())
        checks.append(check("deb_architecture", "FAIL" if issues else "PASS",
                            "; ".join(issues) if issues else f"Architecture={fields['Architecture']}"))
        deps = fields.get("Depends", "")
        missing = [d for d in REQUIRED_DEPENDS if d not in depend_names(deps)]
        checks.append(check("deb_depends", "FAIL" if (not deps or missing) else "PASS",
                            f"Depends={deps!r} missing={missing}"))

        # Control archive: conffiles must mark the PAM file as a user-editable conffile.
        control_dir = work / "control"
        ctl = run(["dpkg-deb", "-e", str(deb), str(control_dir)])
        cf = control_dir / "conffiles"
        if ctl.returncode != 0:
            checks.append(check("deb_conffiles", "FAIL", f"dpkg-deb -e exit {ctl.returncode}"))
        elif not cf.is_file():
            checks.append(check("deb_conffiles", "FAIL", "DEBIAN/conffiles missing"))
        else:
            problems = check_conffiles(cf.read_text())
            checks.append(check("deb_conffiles", "PASS" if not problems else "FAIL",
                                CONFFILES.strip() if not problems else "; ".join(problems)))

    # 2. Contents.
    contents = run(["dpkg-deb", "--contents", str(deb)])
    entries = parse_contents(contents.stdout) if contents.returncode == 0 else []
    if contents.returncode != 0:
        checks.append(check("deb_contents", "FAIL", f"dpkg-deb --contents exit {contents.returncode}"))
    else:
        problems = check_binary_modes(entries)
        checks.append(check("binaries_mode_owner", "PASS" if not problems else "FAIL",
                            "9 binaries 0755 root:root" if not problems else "; ".join(problems)))
        leaks = find_leaks([p for _, _, p in entries])
        checks.append(check("no_path_leakage", "PASS" if not leaks else "FAIL",
                            f"leaking paths: {leaks[:10]}" if leaks else "no build/home paths"))
        checks.extend(check_shipped_assets(entries))

    # 3. Extraction and required data files.
    tree = work / "tree"
    extracted = run(["dpkg-deb", "-x", str(deb), str(tree)])
    if extracted.returncode != 0:
        checks.append(check("deb_extract", "FAIL", f"dpkg-deb -x exit {extracted.returncode}"))
    else:
        checks.append(check("deb_extract", "PASS", "dpkg-deb -x exit 0"))
        for cid, rel in (("xsession_present", XSESSION_PATH), ("pam_present", PAM_PATH)):
            ok = (tree / rel).is_file()
            checks.append(check(cid, "PASS" if ok else "FAIL", rel))
        exec_src = tree / XSESSION_PATH
        if exec_src.is_file():
            exec_line = [ln for ln in exec_src.read_text().splitlines() if ln.startswith("Exec=")]
            ok = exec_line == ["Exec=/usr/bin/flamewm-session"]
            checks.append(check("xsession_exec", "PASS" if ok else "FAIL", f"{exec_line}"))

        # 4. ELF probes.
        for name in BINARIES:
            path = tree / "usr/bin" / name
            if not path.is_file():
                checks.append(check(f"elf_{name}", "FAIL", "missing from extracted tree"))
                continue
            ldd_status, ldd_detail = check_ldd(str(path))
            run_status, run_detail = probe_binary(str(path))
            overall = "FAIL" if "FAIL" in (ldd_status, run_status) else (
                "PASS" if ldd_status == "PASS" and run_status == "PASS" else "NOT_RUN")
            checks.append(check(f"elf_{name}", overall, f"ldd: {ldd_detail}; run: {run_detail}"))

    # 5. SHA256SUMS next to the deb.
    sums = deb.parent / "SHA256SUMS"
    if not sums.is_file():
        checks.append(check("sha256sums", "FAIL", f"{sums} missing: packaging did not finish"))
    else:
        table = parse_sha256sums(sums.read_text())
        want = table.get(deb.name)
        if want is None:
            checks.append(check("sha256sums", "FAIL", f"no entry for {deb.name}"))
        else:
            got = sha256_file(deb)
            checks.append(check("sha256sums", "PASS" if got == want else "FAIL",
                                f"sha256={got}"))

    # 6. dpkg lifecycle under an isolated --root.
    arch_valid = next((c["status"] == "PASS" for c in checks if c["id"] == "deb_architecture"), False)
    if extracted.returncode == 0 and fields and arch_valid:
        checks.extend(dpkg_lifecycle(str(deb), work))
    else:
        checks.append(check("dpkg_lifecycle", "NOT_RUN",
                            "deb architecture invalid, metadata invalid, or extraction unavailable"))

    failed = sum(1 for c in checks if c["status"] == "FAIL")
    report["summary"] = {s: sum(1 for c in checks if c["status"] == s)
                         for s in ("PASS", "FAIL", "NOT_RUN")}
    print(json.dumps(report, indent=2))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
