#!/bin/sh
# End-to-end exercise against this very machine over ssh: a real sshd, a
# real systemd, real rollbacks. Written for the ubuntu CI runner; needs
# passwordless sudo, systemd as PID 1, and permission to start sshd.
set -eu

SLIPWAY=$(pwd)/${SLIPWAY:-target/debug/slipway}
APP=e2e-demo
PORT=8123
HOST="$(whoami)@127.0.0.1"
WORK=$(mktemp -d)

say() { printf '\n== %s\n' "$*"; }

fail() {
    printf 'E2E FAILED: %s\n' "$*"
    sudo systemctl status "$APP" --no-pager -l 2>/dev/null || true
    sudo journalctl -u "$APP" -n 30 --no-pager 2>/dev/null || true
    exit 1
}

served() { curl -fsS "http://127.0.0.1:$PORT/index.html" 2>/dev/null || true; }

make_artifact() {
    rm -rf app && mkdir app
    if [ "$1" != "broken" ]; then
        printf '%s' "$1" > app/index.html
    fi
    cat > app/myapp <<SH
#!/bin/sh
cd "\$(dirname "\$0")"
exec python3 -m http.server $PORT
SH
    chmod +x app/myapp
}

say "prepare sshd and a loopback key"
sudo apt-get install -y -qq openssh-server >/dev/null 2>&1 || true
sudo systemctl start ssh 2>/dev/null || sudo systemctl start sshd
mkdir -p ~/.ssh && chmod 700 ~/.ssh
[ -f ~/.ssh/id_ed25519 ] || ssh-keygen -t ed25519 -N '' -f ~/.ssh/id_ed25519 -q
cat ~/.ssh/id_ed25519.pub >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys
ssh-keyscan -H 127.0.0.1 >> ~/.ssh/known_hosts 2>/dev/null
ssh -o BatchMode=yes "$HOST" true || fail "loopback ssh does not work"

cd "$WORK"
cat > slipway.toml <<TOML
app = "$APP"
hosts = ["$HOST"]

[build]
artifact = "app"

[remote]
keep_releases = 2

[service]
exec_start = "{current}/app/myapp"

[healthcheck]
command = "curl -fsS http://127.0.0.1:$PORT/index.html"
retries = 8
delay_ms = 500
TOML

say "first deploy serves v1"
make_artifact v1
"$SLIPWAY" deploy || fail "deploy v1"
[ "$(served)" = "v1" ] || fail "expected v1, got '$(served)'"
sudo systemctl is-active --quiet "$APP" || fail "unit is not active"

say "second deploy serves v2"
make_artifact v2
"$SLIPWAY" deploy || fail "deploy v2"
[ "$(served)" = "v2" ] || fail "expected v2, got '$(served)'"

say "manual rollback returns to v1"
"$SLIPWAY" rollback || fail "rollback"
[ "$(served)" = "v1" ] || fail "expected v1 after rollback, got '$(served)'"

say "third deploy serves v3 and prunes to keep_releases"
make_artifact v3
"$SLIPWAY" deploy || fail "deploy v3"
[ "$(served)" = "v3" ] || fail "expected v3, got '$(served)'"
kept=$(ssh "$HOST" ls -1 "/srv/$APP/releases" | wc -l)
[ "$kept" -eq 2 ] || fail "expected 2 kept releases, got $kept"

say "a release that cannot pass its health check rolls itself back"
make_artifact broken
if "$SLIPWAY" deploy; then fail "broken deploy was reported as success"; fi
[ "$(served)" = "v3" ] || fail "expected v3 after auto-rollback, got '$(served)'"
sudo systemctl is-active --quiet "$APP" || fail "unit is not active after auto-rollback"

say "secrets in env-file mode reach the process environment"
cat >> slipway.toml <<TOML

[secrets]
identity = "./identity.txt"
TOML
"$SLIPWAY" secrets init >/dev/null
PUB=$(grep '# public key' identity.txt | awk '{print $4}')
printf 'recipients = ["%s"]\n' "$PUB" >> slipway.toml
"$SLIPWAY" secrets init >/dev/null
printf '#!/bin/sh\nprintf "E2E_MARKER=first\\n" > "$1"\n' > ed.sh && chmod +x ed.sh
EDITOR=./ed.sh "$SLIPWAY" secrets edit >/dev/null
make_artifact v4
"$SLIPWAY" deploy || fail "deploy with secrets"
pid=$(sudo systemctl show "$APP" -p MainPID --value)
sudo tr '\0' '\n' < "/proc/$pid/environ" | grep -qx 'E2E_MARKER=first' \
    || fail "secret not in the service environment"

say "secrets push rotates without a redeploy"
printf '#!/bin/sh\nprintf "E2E_MARKER=rotated\\n" > "$1"\n' > ed.sh
EDITOR=./ed.sh "$SLIPWAY" secrets edit >/dev/null
"$SLIPWAY" secrets push || fail "secrets push"
pid=$(sudo systemctl show "$APP" -p MainPID --value)
sudo tr '\0' '\n' < "/proc/$pid/environ" | grep -qx 'E2E_MARKER=rotated' \
    || fail "rotated secret not in the service environment"

say "encrypted-credential mode decrypts through systemd"
printf 'mode = "encrypted-credential"\n' >> slipway.toml
"$SLIPWAY" deploy || fail "deploy with encrypted credentials"
sudo test -f "/srv/$APP/shared/secrets.env.cred" || fail "no .cred file on the host"
sudo systemctl is-active --quiet "$APP" || fail "unit did not start with LoadCredentialEncrypted"

say "status, logs, exec"
"$SLIPWAY" status | grep -q "active" || fail "status does not report active"
"$SLIPWAY" logs -n 5 >/dev/null || fail "logs"
"$SLIPWAY" exec -- echo from-fleet | grep -q from-fleet || fail "exec"

say "user scope deploys without sudo"
sudo loginctl enable-linger "$(whoami)"
for _ in $(seq 20); do [ -S "/run/user/$(id -u)/systemd/private" ] && break; sleep 0.5; done
UPORT=8124
mkdir user && cd user
cat > slipway.toml <<TOML
app = "e2e-user"
hosts = ["$HOST"]

[build]
artifact = "app"

[remote]
root = "$HOME/e2e-user"

[service]
exec_start = "{current}/app/myapp"
scope = "user"

[healthcheck]
command = "curl -fsS http://127.0.0.1:$UPORT/index.html"
retries = 8
delay_ms = 500
TOML
rm -rf app && mkdir app
printf 'u1' > app/index.html
printf '#!/bin/sh\ncd "$(dirname "$0")"\nexec python3 -m http.server %s\n' "$UPORT" > app/myapp
chmod +x app/myapp
"$SLIPWAY" deploy || fail "user-scope deploy"
[ "$(curl -fsS http://127.0.0.1:$UPORT/index.html)" = "u1" ] || fail "user-scope content"
XDG_RUNTIME_DIR="/run/user/$(id -u)" systemctl --user is-active --quiet e2e-user \
    || fail "user unit is not active"

say "all end-to-end checks passed"
