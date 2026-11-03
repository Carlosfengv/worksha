"""Remove owned macOS test bundles and their exact Launch Services records.

Default: preview only. Never stop processes or visit application data directories.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import time
from datetime import datetime, timezone


ROOT = Path(__file__).resolve().parent.parent
SCOPES = ("target/debug/bundle/macos", "target/release/bundle/macos", ".work", "output")
LSREGISTER = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
IDENTIFIER = re.compile(r"app\.localagent\.[A-Za-z0-9.-]+\Z")


def command(args):
    return subprocess.run(
        args, check=True, capture_output=True, text=True, timeout=60
    ).stdout


def owned_path(root, app):
    """Allow only literal .app directories within generated-artifact locations."""
    app = Path(os.path.abspath(app))
    return (
        app.suffix == ".app"
        and app.resolve() == app
        and any(app.is_relative_to(root / scope) for scope in SCOPES)
        and not any(p.suffix == ".app" for p in app.relative_to(root).parents)
    )


def read_app(root, app):
    if not owned_path(root, app) or not app.is_dir():
        return None
    info = app / "Contents/Info.plist"
    if info.resolve() != info:
        return None
    try:
        data = plistlib.loads(info.read_bytes())
    except (OSError, plistlib.InvalidFileException, ValueError):
        return None
    identifier, executable = data.get("CFBundleIdentifier", ""), data.get("CFBundleExecutable", "")
    if not isinstance(identifier, str) or not IDENTIFIER.fullmatch(identifier):
        return None
    if not isinstance(executable, str) or not executable or Path(executable).name != executable:
        return None
    binaries = [app / "Contents/MacOS" / executable, app / "Contents/MacOS/local-agent-service"]
    if any(not p.is_file() or p.resolve() != p for p in binaries):
        return None
    entries, size, newest = [], 0, 0
    for directory, dirs, files in os.walk(app, followlinks=False):
        dirs.sort()
        for item in [Path(directory), *(Path(directory) / n for n in sorted(dirs + files))]:
            s = item.lstat()
            entries.append((str(item.relative_to(app)), s.st_dev, s.st_ino, s.st_mode, s.st_size, s.st_mtime_ns, s.st_ctime_ns))
            newest = max(newest, s.st_mtime, s.st_ctime)
            if item.name in files and stat.S_ISREG(s.st_mode):
                size += s.st_size
    return {
        "path": str(app), "identifier": identifier,
        "name": data.get("CFBundleDisplayName", data.get("CFBundleName", app.stem)),
        "version": data.get("CFBundleShortVersionString"), "executable": executable,
        "bytes": size, "newestChange": newest,
        "fingerprint": hashlib.sha256(json.dumps(entries, sort_keys=True).encode()).hexdigest(),
    }


def discover_apps(root):
    apps = []
    for scope in SCOPES:
        base = root / scope
        if base.resolve() != base or not base.is_dir():
            continue
        for directory, dirs, _ in os.walk(base, followlinks=False):
            for name in list(dirs):
                if name in {"node_modules", ".git"}:
                    dirs.remove(name)
                elif name.endswith(".app"):
                    dirs.remove(name)
                    app = read_app(root, Path(directory) / name)
                    if app:
                        apps.append(app)
    return sorted(apps, key=lambda app: app["path"])


def parse_running(output):
    return {
        str(Path(match.group(1)).resolve())
        for line in output.splitlines()
        if (match := re.match(r"\s*\d+\s+(/.*?\.app)/Contents/", line))
    }


def running_apps():
    # Any executable in a bundle protects it, including a renamed desktop or sidecar.
    return parse_running(command(["/bin/ps", "-axww", "-o", "pid=,comm="]))


def parse_registrations(output):
    records = []
    for block in re.split(r"\n-+\n", output):
        def field(name):
            match = re.search(r"(?m)^\s*" + name + r":\s*(.+)$", block)
            return match.group(1) if match else ""
        identifier = field("identifier")
        app = re.sub(r" \(0x[0-9a-fA-F]+\)$", "", field("path"))
        if IDENTIFIER.fullmatch(identifier) and Path(app).is_absolute() and Path(app).suffix == ".app":
            records.append({"path": app, "identifier": identifier})
    return records


def registrations():
    return parse_registrations(command([LSREGISTER, "-dump"]))


def make_plan(root, records, running, selected=(), keep=(), min_age_minutes=60, now=None):
    now = time.time() if now is None else now
    selected, keep = set(map(str, selected)), set(map(str, keep))
    remove, preserve = [], []
    for app in discover_apps(root):
        p = app["path"]
        reason = (
            "running" if p in running else
            "explicitly-kept" if p in keep else
            "outside-selection" if selected and p not in selected else
            "recently-created-or-changed" if now - app["newestChange"] < min_age_minutes * 60 else None
        )
        (preserve if reason else remove).append({**app, **({"reason": reason} if reason else {})})
    stale = [r for r in records if
             not os.path.lexists(r["path"])
             and str(Path(r["path"]).resolve()) not in running | keep
             and (not selected or str(Path(r["path"]).resolve()) in selected)]
    return {
        "createdAt": datetime.now(timezone.utc).isoformat(), "root": str(root),
        "minAgeMinutes": min_age_minutes, "remove": remove, "preserve": preserve,
        "staleRegistrations": stale, "removed": [], "unregistered": [], "skipped": [], "errors": [],
    }


def sha256(file):
    with file.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest() if hasattr(hashlib, "file_digest") else _hash_stream(stream)


def _hash_stream(stream):
    digest = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(chunk)
    return digest.hexdigest()


def apply_plan(root, plan, records, save):
    if not shutil.rmtree.avoids_symlink_attacks:
        raise RuntimeError("This Python cannot safely remove directory trees")
    for expected in plan["remove"]:
        app = Path(expected["path"])
        try:
            current = read_app(root, app)
            if not current or current["fingerprint"] != expected["fingerprint"] or str(app) in running_apps():
                plan["skipped"].append({"path": str(app), "reason": "changed-or-running"})
                continue
            expected["binarySha256"] = {n: sha256(app / "Contents/MacOS" / n) for n in {expected["executable"], "local-agent-service"}}
            save()
            # Recheck after hashing, immediately before the only deletion.
            current = read_app(root, app)
            if not current or current["fingerprint"] != expected["fingerprint"] or str(app) in running_apps():
                plan["skipped"].append({"path": str(app), "reason": "changed-or-running"})
                continue
            shutil.rmtree(app)
            plan["removed"].append(str(app))
            save()
            # A still-existing bundle can be rediscovered after unregistering it.
            # Remove it first, then unregister only the exact historical paths.
            for record in records:
                if Path(record["path"]).resolve() == app:
                    command([LSREGISTER, "-u", record["path"]])
                    plan["unregistered"].append(record)
        except (OSError, RuntimeError, subprocess.SubprocessError) as error:
            plan["errors"].append({"path": str(app), "error": str(error)})
            raise
        finally:
            save()
    for record in plan["staleRegistrations"]:
        try:
            if os.path.lexists(record["path"]) or str(Path(record["path"]).resolve()) in running_apps():
                plan["skipped"].append({**record, "reason": "reappeared-or-running"})
                continue
            command([LSREGISTER, "-u", record["path"]])
            plan["unregistered"].append(record)
        except (OSError, subprocess.SubprocessError) as error:
            plan["errors"].append({**record, "error": str(error)})
            raise
        finally:
            save()
    after = registrations()
    # Preserve discoverability of the live app even when old copies shared its ID.
    for app in running_apps():
        if read_app(root, Path(app)) and not any(Path(r["path"]).resolve() == Path(app) for r in after):
            command([LSREGISTER, "-f", app])
    targets = set(plan["removed"]) | {str(Path(r["path"]).resolve()) for r in plan["unregistered"]}
    targets -= {s["path"] for s in plan["skipped"]}
    plan["remainingTargetRegistrations"] = [r for r in registrations() if str(Path(r["path"]).resolve()) in targets]
    # An asynchronous indexer may have queued a registration before file removal.
    # Retry only missing target paths once, then report any unresolved records.
    plan["registrationRetries"] = []
    for record in plan["remainingTargetRegistrations"]:
        if not os.path.lexists(record["path"]) and str(Path(record["path"]).resolve()) not in running_apps():
            command([LSREGISTER, "-u", record["path"]])
            plan["registrationRetries"].append(record)
            if record not in plan["unregistered"]:
                plan["unregistered"].append(record)
            save()
    if plan["registrationRetries"]:
        plan["remainingTargetRegistrations"] = [r for r in registrations() if str(Path(r["path"]).resolve()) in targets]
    plan["remainingApps"] = [{"path": a["path"], "identifier": a["identifier"]} for a in discover_apps(root)]
    save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="delete the previewed class of bundles; default is read-only")
    parser.add_argument("--app", action="append", default=[], help="limit to exact owned bundle path; repeat for build and review copies")
    parser.add_argument("--keep", action="append", default=[], help="preserve an exact bundle path")
    parser.add_argument("--min-age-minutes", type=float, default=60, help="skip recently changed bundles (default 60; use 0 for an owned, finished test)")
    parser.add_argument("--report", help="new JSON report path outside any .app; never overwrites an existing file")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Launch Services cleanup requires macOS")
    if not 0 <= args.min_age_minutes < float("inf"):
        parser.error("--min-age-minutes must be finite and nonnegative")
    selected = [Path(os.path.abspath(p)) for p in args.app]
    if any(not owned_path(ROOT, p) for p in selected):
        parser.error("--app must be a nonsymlink .app path inside this project's artifact directories")
    keep = [Path(p).resolve() for p in args.keep]
    report = Path(os.path.abspath(args.report)) if args.report else ROOT / ".work/test-app-cleanup" / (datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ") + ".json")
    if report.resolve() != report or any(p.suffix == ".app" for p in [report, *report.parents]):
        parser.error("report must be outside all .app bundles and symlink paths")
    # Independent cleanup invocations must not race each other.
    lock_dir = ROOT / ".work"
    if lock_dir.resolve() != lock_dir:
        parser.error(".work must not be a symbolic link")
    lock_dir.mkdir(exist_ok=True)
    with os.fdopen(os.open(lock_dir / "test-app-cleanup.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600), "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        records = registrations()
        plan = make_plan(ROOT, records, running_apps(), selected, keep, args.min_age_minutes)
        plan["mode"] = "apply" if args.apply else "preview"
        report.parent.mkdir(parents=True, exist_ok=True)
        with report.open("x") as stream:
            json.dump(plan, stream, ensure_ascii=False, indent=2)
            stream.write("\n")

        def save():
            with report.open("w") as stream:
                json.dump(plan, stream, ensure_ascii=False, indent=2)
                stream.write("\n")

        if args.apply:
            try:
                apply_plan(ROOT, plan, records, save)
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                if not plan["errors"]:
                    plan["errors"].append({"stage": "verification", "error": str(error)})
                save()
        print(json.dumps({
            "mode": plan["mode"], "candidates": len(plan["remove"]),
            "preserved": len(plan["preserve"]), "staleRegistrations": len(plan["staleRegistrations"]),
            "removed": len(plan["removed"]), "unregistered": len(plan["unregistered"]),
            "skipped": len(plan["skipped"]), "errors": plan["errors"],
            "remainingTargetRegistrations": len(plan.get("remainingTargetRegistrations", [])),
            "report": str(report),
        }, ensure_ascii=False, indent=2))
        return 1 if plan["errors"] or plan.get("remainingTargetRegistrations") else 0


if __name__ == "__main__":
    sys.exit(main())
