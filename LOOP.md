# LOOP — autonomous protocol for the orchestrator

One iteration = one task moved to done. Never idle between iterations; the Ralph loop re-feeds this
file whenever you stop, so just keep going.

Each iteration:
1. `cat STATUS.md` and `git status --short`. Uncommitted changes mean a task is in flight or was cut
   off: finish that first. Otherwise take the first unchecked box in PLAN.md (follow its Execution
   order), split it into a task that fits one agent session, and name it in STATUS.md.
2. Write `briefs/M<n>-T<k>.md`: goal, constraints, DECISIONS IDs that apply, files likely involved
   (from `git ls-files`, not from reading them), exact acceptance commands with expected results,
   and "report <=15 lines, do not commit".
3. Dispatch to the `kernel-dev` agent (Sonnet) with the brief path. Do not write or read source yourself.
4. Verify independently: `gmake check` plus any new targets; look at screenshots for anything visual.
   Trust exit codes and pixels, not the report.
5. On failure, resume the same agent with SendMessage and the failure evidence (max 2 retries). On
   the 3rd failure, split the task in PLAN.md or record a blocker in STATUS.md and move to the next
   independent task. For unsafe-heavy work (memory, scheduler, drivers, IPC, crypto, TLS) run
   `kernel-review` (Sonnet) on the diff before accepting and send its findings to the same kernel-dev
   agent with SendMessage.
6. Tick the PLAN.md box, update STATUS.md (done line, next task, notes), append one row to
   docs/BUILDLOG.md (date, task, tests, what review or testing caught), and at a milestone close copy
   the gate screenshot to artifacts/keep/M<n>.png. Then commit and push:
   `git add -A && git commit -q -m "M<n>-T<k>: <what>" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" && git push -q`
7. Every 5 iterations re-read PLAN.md in full, reconcile scope, prune stale STATUS.md notes.

Usage limits (D26):
- An agent that returns "You've hit your session limit · resets <time>" has not failed. Do not start a
  replacement. Wait for the reset with foreground `python3 -c 'import time; time.sleep(540)'` calls,
  then SendMessage the same agent: "the limit has reset, continue exactly where you left off".
- "You've hit your weekly limit · resets <date>" on Sonnet: do not wait days. Re-dispatch the same brief to
  `kernel-dev` with the Agent tool's `model: "haiku"` override, telling it what partial work the cut-off agent
  left. Reviews likewise run on Haiku; the orchestrator spot-checks the riskiest parts of unsafe-heavy
  diffs itself. Return to Sonnet after the reset. Never move implementation onto the orchestrator's model.
- If the orchestrator itself is cut off, the session heartbeat re-prompts it once the session is idle.
  Run `CronList` at the start of each milestone; if the heartbeat is gone (7-day expiry), re-create it
  with CronCreate, cron "13,43 * * * *", prompt "OtterOS heartbeat: continue the loop per LOOP.md".

Token rules for the orchestrator:
- Read summaries, exit codes and screenshots, never source (unless a task failed twice).
- Briefs are self-contained; the agent starts with zero context.
- Never pull whole logs into context: `tail -20`, `grep -n` and the PNG are enough.

Human-in-the-loop points (record in STATUS.md, never block on them): the hardware smoke test and the
laptop details (docs/HARDWARE.md), the API key file for agent mode, recording the final demo.
Everything else is your call; record non-obvious calls in DECISIONS.md.

If every remaining PLAN.md task is blocked on the human, write the blockers into STATUS.md, commit,
and end the loop by deleting `.claude/ralph-loop.local.md`; never spin idle.

Completion: output `<promise>OTTEROS COMPLETE</promise>` only when every PLAN.md box is ticked and the
human has confirmed the real-hardware demo. Never output it otherwise.
