#!/usr/bin/env python3
"""macOS: exercise actual ENOSPC only inside a disposable 32 MiB disk image."""
import errno
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import tempfile
import uuid

root = pathlib.Path(tempfile.mkdtemp(prefix="local-agent-export-full-")).resolve()
mount = root / "volume"
mount.mkdir()
image = root / "fixture.dmg"
device = None
try:
    subprocess.run(["hdiutil", "create", "-size", "32m", "-fs", "HFS+", "-volname", "Export Test Fixture", "-type", "UDIF", "-nospotlight", str(image)], check=True)
    reply = subprocess.check_output(["hdiutil", "attach", "-plist", "-nobrowse", "-mountpoint", str(mount), str(image)])
    entities = plistlib.loads(reply)["system-entities"]
    device = next(item["dev-entry"] for item in entities if re.fullmatch(r"/dev/disk\d+", item.get("dev-entry", "")))
    mounted = next(item for item in entities if pathlib.Path(item.get("mount-point", "/nonexistent")).resolve() == mount)
    assert mount.stat().st_dev != root.stat().st_dev, "refuse to fill the workspace filesystem"
    marker = str(uuid.uuid4())
    (mount / ".local-agent-export-fixture").write_text(marker)
    (mount / "requests.csv").write_text("original")
    filler = mount / "filler"
    exhausted = False
    with filler.open("wb", buffering=0) as stream:
        try:
            for _ in range(64):
                stream.write(b"x" * (1024 * 1024))
        except OSError as error:
            if error.errno != errno.ENOSPC:
                raise
            exhausted = True
        assert exhausted, "refuse to write beyond the bounded disk-image fixture"
        stream.truncate(max(0, stream.tell() - 1024 * 1024))
        os.fsync(stream.fileno())
    env = dict(os.environ, EXPORT_TEST_FULL_DIR=str(mount), EXPORT_TEST_FULL_MARKER=marker)
    subprocess.run(["cargo", "test", "--locked", "-p", "local-agent-desktop", "real_disk_full_removes", "--", "--ignored", "--nocapture"], env=env, check=True)
    assert (mount / "requests.csv").read_text() == "original"
    assert not list(mount.glob(".requests-*.tmp"))
    print("Actual ENOSPC passed: destination unchanged, partial CSV removed, temporary service resource cancelled.")
finally:
    detached = device is None
    if device:
        result = subprocess.run(["hdiutil", "detach", device], check=False)
        detached = result.returncode == 0
    if detached:
        shutil.rmtree(root)
    else:
        print("Owned fixture remains mounted; detach this device before removing its directory:", device, root)
