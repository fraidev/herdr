---
title: Multi-runtime federation
description: Hub session registry for local and remote Herdr agents
---

# Multi-runtime federation

Herdr can act as a **hub** that lists agents from multiple Herdr servers
(local session + remote hosts) without merging PTYs across machines.

This feature is **unreleased** preview behavior.

## Concepts

| Term | Meaning |
| --- | --- |
| **Hub** | Local `herdr server` that owns the runtime registry |
| **Runtime** | A Herdr server process (local always present; remotes optional) |
| **Socket runtime** | Another session on the same machine via its `herdr.sock` |
| **Remote SSH runtime** | A remote host's `herdr server` reached over an SSH API bridge |

Remote agents keep running on their own host. The hub only tunnels JSON API
calls and projects inventory into `agent.list` and the sidebar agents panel.

## CLI

```bash
# Membership (offline until connect)
herdr runtime list
herdr runtime add worker --socket /path/to/sessions/worker/herdr.sock --label Worker
herdr runtime add workbox --ssh workbox.example --session worker --label Workbox
herdr runtime get workbox
herdr runtime remove workbox

# Live tunnel
herdr runtime connect worker
herdr runtime disconnect worker

# Aggregated inventory (local + connected remotes)
herdr agent list
# remote agents appear with runtime_id and scoped names, e.g. workbox/reviewer

# Control proxy (scoped targets)
herdr agent focus workbox/reviewer
herdr agent prompt workbox/reviewer "hello"
```

## API

Additive methods:

- `runtime.list` / `runtime.get` / `runtime.add` / `runtime.remove`
- `runtime.connect` / `runtime.disconnect`
- `agent.list` includes `runtime_id` and optional `runtime_label` on each agent

## Sidebar

The agents panel projects hub inventory: local agents first, then mirrored
remote agents with host labels. Spaces remain local-only in v1.

**Remote focus MVP:** selecting a remote agent proxies `agent.focus` through the
hub to the remote API. It does **not** embed a multi-pane remote layout yet;
full stream attach / nested chrome is future work.

## Failure behavior

- SSH or remote server drop marks that runtime offline/degraded
- Local agents keep working
- Last successful remote agent snapshot may be retained while degraded

## Testing without SSH

Use two named sessions on one machine:

```bash
HERDR_SESSION=hub herdr
HERDR_SESSION=worker herdr   # separate terminal
# then on hub:
herdr runtime add worker --socket "$(herdr --session worker status --json | jq -r ...)"
herdr runtime connect worker
herdr agent list
```

Prefer temp sessions for automation; do not require Orb/Lima in CI.
