# Claude Code Instructions

## !!!MANDATORY FIRST ACTION!!!

**STOP. Before responding to the user, ask yourself: "Do I have the contents of `~/.claude-config/startup.md` in my current context?"**

If NO → Use the Read tool to read `~/.claude-config/startup.md` RIGHT NOW, before doing anything else.
If YES → Proceed normally.

This applies after every `/compact`, session start, or context reset. The file contains critical workflow rules and preferences.

---

## CRITICAL: Process Safety

**NEVER use `cargo build` without `-j 2`.** Unrestricted parallel compilation triggers the Linux OOM killer, which indiscriminately murders other processes — including the user's tmux sessions, the server team's tmux sessions, and anything else the kernel decides to sacrifice.

**Use the project scripts:**
- **Stop the client:** `./scripts/stop-client.sh` — graceful API shutdown → SIGTERM → warn (NEVER SIGKILL)
- **Build the client:** `./scripts/build.sh` — safe `-j 2` build (or `cargo build -j 2`)

**NEVER do any of the following:**
- `cargo build` without `-j 2` (or using `scripts/build.sh`)
- `kill -9` or `SIGKILL` any process — you don't know what else it will take down
- `pkill` or `killall` with broad patterns — too dangerous, collateral damage
- `fuser -k` — same problem, kills indiscriminately
- Any command that could trigger OOM (unrestricted parallel builds, loading huge files into memory)

**If you need to stop a process:** Use the stop script or targeted `kill -TERM <specific_pid>`. If it won't die after SIGTERM, **ask the user** — do NOT escalate to SIGKILL.

