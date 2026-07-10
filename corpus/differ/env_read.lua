-- DIFFER: from=5.2 targets=5.1
-- Lowering rule: env — reading _ENV becomes getfenv(1); at the main chunk
-- both denote the global environment.
print(_ENV == _G)
_ENV.marker = 7
print(marker)
print("ok")
