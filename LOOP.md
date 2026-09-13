# LOOP — autonomous protocol for the orchestrator (Fable 5.1)

One iteration = one task moved to done. Never idle between iterations; the Ralph loop re-feeds this
file whenever you stop, so just keep going.

Each iteration:
1. `cat STATUS.md`. If the next task is unclear, take the first unchecked box in PLAN.md, split it into
   a task that fits one agent session, and write it into STATUS.md.
2. Write `briefs/M<n>-T<k>.md`: goal, constraints, DECISIONS IDs that apply, files likely involved
   (from `git ls-files`, not from reading them), exact acceptance commands with expected results,
   and "report <=15 lines, do not commit".
3. Dispatch to the `kernel-dev` agent (Sonnet) with the brief path, in the foreground. Do not write
   or read source yourself.
4. Verify independently: run the acceptance commands (`gmake test`, `gmake shot` then view the PNG,
   scripted checks). Trust exit codes and pixels, not the report.
5. On failure: re-dispatch with the failure evidence attached (max 2 retries). On the 3rd failure,
   split the task in PLAN.md or record a blocker in STATUS.md and move to the next independent task.
   For `unsafe`-heavy work (memory, scheduler, drivers, IPC) also run `kernel-review` (Sonnet) on
   `git diff` before accepting; feed its REJECT/fix findings back to kernel-dev.
6. Tick the PLAN.md box, update STATUS.md (done line, next task, notes), then
   `git add -A && git commit -q -m "M<n>-T<k>: <what>" && git push -q`.
7. Every 5 iterations re-read PLAN.md in full, reconcile scope, prune stale STATUS.md notes.

Token rules for the orchestrator:
- Read summaries, exit codes and screenshots, never source (unless a task failed twice).
- Briefs are self-contained; the agent starts with zero context.
- Never pull whole logs into context: `tail -20`, `grep -n`, and the PNG are enough.
- Extract the acceptance lines from long agent reports and move on.

Human-in-the-loop points (record in STATUS.md, never block on them): publishing the repo, the OS
name, the M6 laptop. Everything else is your call; record non-obvious calls in DECISIONS.md.

If every remaining PLAN.md task is blocked on the human (e.g. M6 hardware), write the blockers into
STATUS.md, commit, and end the loop by deleting `.claude/ralph-loop.local.md`; never spin idle.

Completion: output `<promise>OTTEROS M6 DONE</promise>` only when every PLAN.md box is ticked and
the human has confirmed the real-hardware boot. Never output it otherwise.
