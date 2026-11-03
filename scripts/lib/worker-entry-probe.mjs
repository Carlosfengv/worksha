import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

// Test-only process-entry observer. The live service keeps executing the exact
// copied Mach-O; its owned launch path is replaced after readiness. Every child
// entry is logged synchronously before exec of the same original worker bytes.
// Positive controls must prove interception before any zero-start assertion.
export function workerEntryProbe(binary, root) {
  assert.equal(process.platform, "darwin");
  const entry = path.join(root, "service-entry");
  const real = path.join(root, "service-real");
  const launcher = path.join(root, "worker-launcher");
  const log = path.join(root, "worker-entries.jsonl");
  const source = path.join(root, "worker-launcher.c");
  fs.copyFileSync(binary, entry, fs.constants.COPYFILE_EXCL);
  fs.chmodSync(entry, 0o700);
  fs.writeFileSync(log, "", { flag: "wx", mode: 0o600 });
  fs.writeFileSync(
    source,
    `#include <unistd.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
int main(int argc,char **argv) {
  const char *real=${JSON.stringify(real)};
  const char *log=${JSON.stringify(log)};
  int fd=open(log,O_WRONLY|O_APPEND|O_NOFOLLOW|O_CLOEXEC);
  if(fd<0)return 126;
  const char *kind=argc>1&&!strcmp(argv[1],"--plugin-worker")?"plugin":"unexpected";
  char line[160];int length=snprintf(line,sizeof line,"{\\"pid\\":%d,\\"kind\\":\\"%s\\",\\"phase\\":\\"entry\\"}\\n",getpid(),kind);
  if(length<0||length>=(int)sizeof line||write(fd,line,length)!=length)return 126;
  close(fd);
  argv[0]=(char*)real;execv(real,argv);
  fd=open(log,O_WRONLY|O_APPEND|O_NOFOLLOW|O_CLOEXEC);
  if(fd>=0){length=snprintf(line,sizeof line,"{\\"pid\\":%d,\\"phase\\":\\"exec-failed\\",\\"errno\\":%d}\\n",getpid(),errno);write(fd,line,length);close(fd);}
  return 127;
}
`,
    { flag: "wx", mode: 0o600 },
  );
  execFileSync(
    "/usr/bin/clang",
    [source, "-O2", "-Wall", "-Wextra", "-Werror", "-o", launcher],
    { stdio: "pipe" },
  );
  return {
    entry,
    real,
    log,
    install() {
      fs.renameSync(entry, real);
      fs.renameSync(launcher, entry);
    },
    events() {
      const text = fs.readFileSync(log, "utf8");
      assert(!text || text.endsWith("\n"), "incomplete observer frame");
      return text.split("\n").filter(Boolean).map(JSON.parse);
    },
    command(pid) {
      try {
        return execFileSync("/bin/ps", ["-p", String(pid), "-o", "command="], {
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
        }).trim();
      } catch (error) {
        if (
          error.status === 1 &&
          !String(error.stdout).trim() &&
          !String(error.stderr).trim()
        )
          return null;
        throw error;
      }
    },
  };
}
