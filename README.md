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
  the unit file; with `sudo = false` connect as root instead — or set
  `service.scope = "user"` and skip privileges entirely (run
  `loginctl enable-linger` once so the service survives logout).

## Install

Grab a binary from [releases](https://github.com/grahov/slipway/releases)
(linux and macOS, x86_64 and aarch64), or install from crates.io:

```
cargo install slipway
```

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
| `slipway logs` | recent journal entries per host; `-f` follows one host |
| `slipway exec -- CMD` | run a command on every host through the remote shell |
| `slipway secrets init` | create the age identity, then the encrypted secrets file |
| `slipway secrets edit` | decrypt into `$EDITOR`, validate, re-encrypt |
| `slipway secrets push` | rotate secrets: upload, restart, health-check |

`deploy` also takes `--dry-run` (print commands, execute nothing) and
`--skip-build` (deploy the artifact as it is). Every fleet command accepts
`-c FILE`, `--host SUBSTRING`, and `--group NAME` to work on part of the
fleet.

## Config reference

| Key | Default | Meaning |
| --- | --- | --- |
| `app` | — | service and unit name; `[a-z0-9._-]` |
| `hosts` | — | ssh destinations, passed to `ssh` verbatim; a host may also be a table `{ addr, group, sudo, root }` overriding the defaults below |
| `build.command` | none | local build, run once per deploy via `sh -c` |
| `build.artifact` | — | file or directory uploaded as the release |
| `remote.root` | `/srv/{app}` | release layout location on the host |
| `remote.keep_releases` | `5` | releases kept after a successful deploy |
| `remote.sudo` | `true` | prefix privileged remote commands with `sudo -n` |
| `rollout.canary` | `0` | hosts that must fully succeed before the rest start |
| `rollout.parallel` | `1` | hosts deployed concurrently within a wave |
| `service.exec_start` | — | `ExecStart=`; `{current}` expands to `{root}/current` |
| `service.scope` | `system` | `user` runs under `systemctl --user`, no sudo anywhere |
| `service.user` | none | `User=` in the unit |
| `service.env` | `{}` | `Environment=` lines |
| `service.unit_extra` | `[]` | verbatim extra `[Service]` lines |
| `healthcheck.command` | none | runs on the host; exit 0 means healthy |
| `healthcheck.retries` | `5` | attempts before the deploy counts as failed |
| `healthcheck.delay_ms` | `1000` | pause between attempts |
| `secrets.file` | `secrets.env.age` | age-encrypted dotenv, committed to the repo |
| `secrets.identity` | `~/.config/slipway/identity.txt` | age identity or OpenSSH private key used to decrypt |
| `secrets.recipients` | `[]` | `age1...` or `ssh-ed25519/ssh-rsa` public keys allowed to re-encrypt |
| `secrets.mode` | `env-file` | `env-file`, `credential`, or `encrypted-credential` |

## How a deploy works

Each host ends up with the classic capistrano layout:

```
/srv/myapp/
  releases/
    20260825120301000/
    20260825131500247/
  current -> releases/20260825131500247
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

The fleet is walked in waves: `rollout.canary` hosts go first and all of
them must pass their health checks before anything else starts;
the rest follow in batches of `rollout.parallel`. The defaults —
no canary, one host at a time — are exactly the sequential behavior you
would script by hand.

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

Three ways for the service to consume it:

- `mode = "env-file"` (default): the unit gets
  `EnvironmentFile={root}/shared/secrets.env` and the variables appear in
  the process environment; no application changes.
- `mode = "credential"`: the unit gets `LoadCredential=secrets.env:...`
  and the application reads `$CREDENTIALS_DIRECTORY/secrets.env` itself —
  the systemd-native way that keeps values out of the environment.
- `mode = "encrypted-credential"`: like `credential`, but the payload is
  piped into `systemd-creds encrypt` on the host, so what sits on disk is
  encrypted with the host's own key (and its TPM when there is one), and
  the unit reads it via `LoadCredentialEncrypted=`. Needs system scope and
  systemd 250+.

The identity may also be an existing OpenSSH private key: point
`secrets.identity` at it and list teammates' `ssh-ed25519 ...` public keys
as recipients. Keys with a passphrase are rejected — slipway never
prompts.

`secrets push` rotates without a redeploy: upload, restart, health-check;
if the check fails, the previous secrets file comes back and the service
restarts again. In the two plaintext modes the host stores the decrypted
file at rest — 0600, owned by the deploy user, readable by PID 1 but not
by the service user; `encrypted-credential` removes even that.

## Rollback, honestly

- Rollback restores the previous binary and the previous secrets file, not
  the world: schema migrations, cache contents, and anything the new
  release wrote stay as they are.
- Hosts within a wave fail independently and each failing host rolls
  itself back; the rollout stops between waves, so hosts in earlier waves
  keep the new release and hosts in later waves were never touched. With
  a canary configured, a canary failure means zero non-canary hosts moved.
- A first deploy has nothing to roll back to; a failing health check there
  is reported and the release is left in place for inspection.

## Roadmap

- multiple services per config file
- `slipway diff`: what a deploy would change, before running it

## License

MIT
