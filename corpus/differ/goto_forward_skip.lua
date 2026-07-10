-- DIFFER: from=5.2 targets=5.1
-- Lowering rule: gotos — a direct unconditional forward goto skips its region
-- (lowered to an always-true skip flag).
print("start")
goto done
print("dead code, must never print")
::done::
print("alive")
