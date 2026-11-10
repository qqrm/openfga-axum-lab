# Experimental single-container browser laboratory

This is **one Linux image** containing three **real** processes:

- PostgreSQL 17 (SQL source, sessions, and the OpenFGA datastore in separate databases)
- OpenFGA 1.10.3 (official release archive checked against a pinned SHA-256)
- The existing Rust/Axum application and `lab-tool`, built with the HTTP adapter

It does **not** contain Docker, a fake authorization engine, JavaScript
reimplementations, or an alternative FGA model. The pinned, unchanged Lakekeeper
model remains the default. It does not modify the normal Compose/Pages deployment.

## Native proof before emulation

Run these commands at the repository root on an x86_64 Docker host:

```sh
docker build --platform linux/amd64 -f browser-single/Dockerfile -t authz-browser-one:local .
docker run --rm -d --name authz-browser-one --shm-size=128m \
  -e BROWSER_LAB_EPHEMERAL=yes -p 127.0.0.1:3000:3000 authz-browser-one:local
docker exec authz-browser-one /usr/local/bin/lab-tool probe
docker exec -e SMOKE_ALLOW_MUTATIONS=true authz-browser-one /usr/local/bin/lab-tool acceptance
```

The entrypoint starts PostgreSQL first, migrates its OpenFGA database, starts
OpenFGA, verifies/binds the exact FGA model, seeds the catalog via actual SQL
migration, then starts Axum. Each process runs as the unprivileged `postgres`
OS user. Both PostgreSQL and OpenFGA bind to guest loopback only. Only Axum is
exposed on container port 3000. The native smoke test asserts actual access
denial, grant, allow, revocation and denial again.

For an interactive local session, read the generated password **locally** with
`docker exec authz-browser-one cat /tmp/browser-lab/demo-password`, open
`http://127.0.0.1:3000`, and log in as one of the demo identities.
Use `docker logs authz-browser-one` for startup diagnostics and
`docker rm -f authz-browser-one` to stop it.

## Size and conversion

`.github/workflows/browser-single.yml` builds and runs the actual one-image
stack on PRs affecting this experiment; its native smoke records the image
size. **Conversion is manual and opt-in**: run *Browser single-image feasibility*
on this branch with `convert=true`. The workflow only converts *after* the
native integration test succeeds. It checks the c2w v0.8.5 release checksums,
calls `c2w --to-js authz-browser-one:ci browser-output/`, records the output
sizes, and only uploads the generated executable bundle if at most 250 MiB.
It never publishes a converted executable to Pages automatically.

**Not yet established:** whether the emulated Linux kernel supports the
PostgreSQL shared-memory/process semantics; whether all three daemons can boot
and complete a Check/Write within browser memory and CPU limits; how to bridge
browser Fetch requests into port 3000 without an external server; and whether
the result works in Firefox/Chrome. A successful `c2w` conversion alone is
**not** a successful browser deployment.

## Deliberate security and durability limits

- `BROWSER_LAB_EPHEMERAL=yes` must be set explicitly. The DB uses trust
  authentication *only on the guest loopback*. Never connect this image to
  public networks or production data.
- Passwords are generated locally on startup, not baked into image layers;
  the generated demo password is stored in the unprivileged user's private
  temporary directory. The browser-side login bridge is **not implemented**.
- Data live under `/tmp` and are not backed up; restart/restore and
  multi-session isolation are **not guaranteed**. This is not a production image.
- The inherited PostgreSQL base image has its own declared volume. This
  experiment overrides `PGDATA` to a separate temporary location.
- Do not infer full-service verification from the existing scalar WASM demo on
  Pages; that demo checks lifecycle rules, not OpenFGA decisions.

Related prior experiment: [docs/browser-feasibility.md](../docs/browser-feasibility.md).
