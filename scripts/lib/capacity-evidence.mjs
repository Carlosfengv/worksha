import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readdirSync, realpathSync, statfsSync, statSync } from "node:fs";
import path from "node:path";

// Free-space samples include unlinked SQLite spills and unrelated volume I/O.
export function physicalVolume(directory) {
  assert.equal(process.platform, "darwin");
  const resolved = realpathSync(directory);
  const device = execFileSync("/bin/df", ["-P", resolved], {
    encoding: "utf8",
  })
    .trim()
    .split("\n")
    .at(-1)
    .trim()
    .split(/\s+/)[0];
  const plist = execFileSync("/usr/sbin/diskutil", ["info", "-plist", device]);
  const info = JSON.parse(
    execFileSync("/usr/bin/plutil", ["-convert", "json", "-o", "-", "-"], {
      input: plist,
    }),
  );
  assert.equal(
    info.FilesystemType,
    "apfs",
    "Use an ordinary APFS disk, not a RAM disk",
  );
  assert.equal(
    info.Internal,
    true,
    "This capacity profile requires the internal physical disk",
  );
  assert.equal(
    statSync(resolved).dev,
    statSync(process.cwd()).dev,
    "Fixture and workspace volumes differ",
  );
  return {
    device: info.DeviceNode,
    filesystem: info.FilesystemType,
    mount: info.MountPoint,
    internal: info.Internal,
    solidState: info.SolidState,
    directory: resolved,
  };
}

export function diskSample(directory) {
  let logicalBytes = 0,
    allocatedBytes = 0;
  const walk = (current) => {
    let entries;
    try {
      entries = readdirSync(current, { withFileTypes: true });
    } catch (error) {
      if (error.code === "ENOENT") return;
      throw error;
    }
    for (const entry of entries) {
      const file = path.join(current, entry.name);
      try {
        if (entry.isDirectory()) walk(file);
        else if (entry.isFile()) {
          const meta = statSync(file);
          logicalBytes += meta.size;
          allocatedBytes += meta.blocks * 512;
        }
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
    }
  };
  walk(directory);
  const volume = statfsSync(directory);
  return {
    logicalBytes,
    allocatedBytes,
    volumeAvailableBytes: volume.bavail * volume.bsize,
  };
}

export function percentile(values, percent) {
  assert(values.length > 0);
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[
    Math.min(sorted.length - 1, Math.ceil((sorted.length * percent) / 100) - 1)
  ];
}
