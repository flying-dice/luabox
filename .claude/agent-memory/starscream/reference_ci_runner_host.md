---
name: reference-ci-runner-host
description: Where the GitLab CI runner for luabox lives (Unraid host `enterprise`), its layout, and the 2026-09-06 disk fix — check here before touching CI caching or diagnosing "No space left" in jobs
metadata:
  type: reference
---

The only untagged GitLab runner (id 3, `unraid-runner`) is a `gitlab/gitlab-runner` docker container on the owner's Unraid box `enterprise.beluga-sirius.ts.net` (root SSH; the owner hands out the password in-session — never store it). Config: `/mnt/cache/appdata/gitlab-runner/config.toml` (backups `config.toml.bak.<epoch>` beside it). Persona-tagged runners 5/6/7 (skywarp/thundercracker/shockwave) are separate.

Layout after the 2026-09-06 fix (issue #85, decision 15):
- `concurrent = 2`; `[runners.docker] volumes` binds `/builds` and `/cache` to `/mnt/cache/appdata/gitlab-runner/{builds,cache}` on the 932 GB NVMe. Before, both were anonymous docker volumes inside the 80 GB `docker.img` (`/mnt/user/system/docker/docker.img`, `DOCKER_IMAGE_SIZE=80`) and `concurrent = 6` — that was the "constant cache problem", not Rust or GitLab.
- Eviction: `/boot/config/scripts/runner-cache-sweep.sh` hourly via `/boot/config/plugins/dynamix/docker-runner-cache-sweep.cron` (owner's rule: untouched 24 h → deleted; 60 GiB cap). Source in repo `scripts/ops/`. Older `docker-runner-prune.sh` (04:00) prunes labelled volumes + all unused images.
- `/boot` is vfat: scripts there cannot be `+x`; cron lines must call `/bin/bash <script>`. `update_cron` merges `*.cron` into `/etc/cron.d/root`.
- Automation is denied destructive docker / `rm` on the host by the auto-mode classifier — hand those to the owner as exact commands; config edits, `scp`, `docker restart`, `update_cron` were allowed.
- Known leftovers for the owner: runner container `Restart=no`; empty `[runners.cache]` block logs `cache factory not found` at start (harmless); `docker inspect` of the runner prints its registration token — never paste it.
