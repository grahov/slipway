<p align="center">
  <img src="assets/logo.svg" width="400" alt="slipway">
</p>

<p align="center"><b>Capistrano-style deploys for plain Linux servers: over ssh, onto systemd.</b></p>

`kamal` ships containers to your servers. slipway is for the fleet that never
grew a container runtime: it builds locally, uploads the artifact over ssh
into a `releases/` layout, writes the systemd unit, flips the `current`
symlink, restarts, health-checks — and rolls itself back when the check
keeps failing.

```
slipway deploy
```

## Why

Small fleets mostly deploy with a hand-me-down script: scp, ssh, systemctl
restart, hope. The script has no releases to roll back to, half-installs
things when a step fails in the middle, and lives in one person's home
directory. slipway replaces it with one binary and **zero server-side
dependencies** — nothing to install on the hosts. If a box runs sshd and
systemd, it is already a deploy target.

## Requirements

- Local: the `ssh` and `tar` binaries. Key or agent auth is required —
  slipway runs ssh in BatchMode and never prompts for passwords. ssh config
  aliases, jump hosts, and multiplexing work as usual.
- Remote: sshd, systemd, tar, GNU coreutils. With the default `sudo = true`
  the ssh user needs passwordless sudo for `systemctl` and for installing
  the unit file; with `sudo = false` connect as root instead.

## Install

```
cargo install --path .
```

Binary releases are planned.

## Quick start

`slipway init` writes a commented `slipway.toml`; trimmed to the essentials
it looks like this:

```toml
app = "myapp"
hosts = ["deploy@app1.example.com", "deploy@app2.example.com"]

[build]
command = "cargo build --release"
artifact = "target/release/myapp"

[service]
exec_start = "{current}/myapp"

[healthcheck]
command = "curl -fsS http://127.0.0.1:8080/health"
```

Then `slipway deploy`. Run `slipway deploy --dry-run` first to see every
command it would execute, verbatim, without touching anything.

## Commands

| Command | Meaning |
| --- | --- |
| `slipway init` | write an example `slipway.toml` |
| `slipway deploy` | build, upload, flip, restart, health-check every host |
| `slipway rollback` | flip hosts back to the release preceding the current one |
| `slipway status` | current release, service state, recent releases per host |
| `slipway secrets init` | create the age identity, then the encrypted secrets file |
| `slipway secrets edit` | decrypt into `$EDITOR`, validate, re-encrypt |
| `slipway secrets push` | rotate secrets: upload, restart, health-check |

`deploy` also takes `--dry-run` (print commands, execute nothing) and
`--skip-build` (deploy the artifact as it is). All three commands accept
`-c FILE` and `--host SUBSTRING` to work on part of the fleet.

## Config reference

| Key | Default | Meaning |
| --- | --- | --- |
| `app` | — | service and unit name; `[a-z0-9._-]` |
| `hosts` | — | ssh destinations, passed to `ssh` verbatim |
| `build.command` | none | local build, run once per deploy via `sh -c` |
| `build.artifact` | — | file or directory uploaded as the release |
| `remote.root` | `/srv/{app}` | release layout location on the host |
| `remote.keep_releases` | `5` | releases kept after a successful deploy |
| `remote.sudo` | `true` | prefix privileged remote commands with `sudo -n` |
| `service.exec_start` | — | `ExecStart=`; `{current}` expands to `{root}/current` |
| `service.user` | none | `User=` in the unit |
| `service.env` | `{}` | `Environment=` lines |
| `service.unit_extra` | `[]` | verbatim extra `[Service]` lines |
| `healthcheck.command` | none | runs on the host; exit 0 means healthy |
| `healthcheck.retries` | `5` | attempts before the deploy counts as failed |
| `healthcheck.delay_ms` | `1000` | pause between attempts |
| `secrets.file` | `secrets.env.age` | age-encrypted dotenv, committed to the repo |
| `secrets.identity` | `~/.config/slipway/identity.txt` | local age identity used to decrypt |
| `secrets.recipients` | `[]` | age public keys allowed to re-encrypt |
| `secrets.mode` | `env-file` | `env-file` or `credential` |

## How a deploy works

Each host ends up with the classic capistrano layout:

```
/srv/myapp/
  releases/
    20260825120301/
    20260825131500/
  current -> releases/20260825131500
```

1. The build command runs locally, once.
2. The artifact streams to the host as a tarball over a single ssh exec and
   unpacks into `releases/{timestamp}`.
3. The systemd unit is rendered from config and installed only when its
   content changed, followed by `daemon-reload` and `enable`.
4. `current` is flipped atomically: a temp symlink, then `mv -Tf`. A crash
   mid-flip leaves the old or the new link, never a broken one.
5. The service restarts and the health check runs on the host until it
   passes or runs out of retries.
6. On success, releases beyond `keep_releases` are pruned. On failure,
   `current` flips back to the previous release, the service restarts, and
   the deploy exits non-zero telling you both what failed and where the
   host ended up.

The unit references only `{root}/current`, so routine deploys never rewrite
it, and `slipway rollback` is nothing more exotic than steps 4-5 aimed at
the previous release.

## Secrets

Deploys usually carry configuration that must not sit in git as plaintext.
slipway's answer is one age-encrypted dotenv file that does sit in git:

```
slipway secrets init
slipway secrets edit
slipway secrets push
```

`init` creates a local identity and prints its public key — put it into
`secrets.recipients` (one entry per teammate) and commit `secrets.env.age`.
Every deploy then decrypts the file locally and installs it to
`{root}/shared/secrets.env` on the host, mode 0600, before the restart.
The identity never leaves your machine. Encryption is the `age` crate
built into slipway — no external binary — and the armored ciphertext
diffs as text in git.

Two ways for the service to consume it:

- `mode = "env-file"` (default): the unit gets
  `EnvironmentFile={root}/shared/secrets.env` and the variables appear in
  the process environment; no application changes.
- `mode = "credential"`: the unit gets `LoadCredential=secrets.env:...`
  and the application reads `$CREDENTIALS_DIRECTORY/secrets.env` itself —
  the systemd-native way that keeps values out of the environment.

`secrets push` rotates without a redeploy: upload, restart, health-check;
if the check fails, the previous secrets file comes back and the service
restarts again. The host does store the decrypted file at rest — 0600,
owned by the deploy user, readable by PID 1 but not by the service user.
Host-bound encryption at rest (`systemd-creds`, TPM) is on the roadmap.

## Rollback, honestly

- Rollback restores the previous binary and the previous secrets file, not
  the world: schema migrations, cache contents, and anything the new
  release wrote stay as they are.
- Hosts deploy sequentially and the run stops at the first failure: hosts
  before it keep the new release, hosts after it were never touched. The
  failing host itself is rolled back automatically.
- A first deploy has nothing to roll back to; a failing health check there
  is reported and the release is left in place for inspection.

## Roadmap

- parallel and canary deploys across the fleet
- `slipway logs` / `slipway exec` passthroughs
- host groups and per-host overrides in the config
- user-level units (`systemctl --user`) for sudo-less deploys
- host-bound secret encryption at rest (`systemd-creds`, TPM)
- ssh keys as age identities for secrets
- binary releases

## License

MIT
