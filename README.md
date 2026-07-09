<img width="1024" height="1024" alt="smoke" src="https://github.com/user-attachments/assets/039a48fc-91e0-4221-8706-e52cf011a832" />
# SMOKE

**Write. Run. Know.**

Status: early development

---

> A PreToolUse hook for Claude Code that runs AI-generated JS/TS and Python code
> in a sandbox before the agent's file write is allowed to complete.
> The agent finds out about bugs the same second it introduces them.

## Security model

**JS/TS execution** is sandboxed by the V8 engine. Code has no filesystem
or network access by default. This is a property of the engine, not of our
configuration.

**Python execution** is process-isolated with resource limits (CPU time,
memory) and a partial seccomp filter (denies fork/exec and raw sockets).
This is NOT a full sandbox:
- Logic-based escapes (`__subclasses__()`, frame manipulation) stay within
  the Python VM and are not prevented by seccomp
- Do not run untrusted third-party Python through SMOKE expecting
  container-grade isolation — use E2B or Modal for that
- SMOKE's Python value is catching bugs in *agent-generated* code before
  they reach disk — code the agent wrote, not adversarial code
