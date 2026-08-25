//! The pipelines: deploy, rollback, status.

use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::release;
use crate::ssh::Shell;
use crate::ui;
use crate::unit;

pub struct Opts {
    pub dry_run: bool,
    pub skip_build: bool,
    pub host_filter: Option<String>,
}

/// One host's ssh session plus the dry-run switch. In dry mode every call
/// prints its exact remote script and reads come back empty; the pipeline
/// treats empty as "absent", so the printed plan is the first-deploy shape
/// and nothing on either side is touched.
struct Runner {
    shell: Shell,
    dry: bool,
    log: Vec<String>,
}

impl Runner {
    fn new(config: &Config, host: &str, dry: bool) -> Self {
        Self {
            shell: Shell::new(host, config.remote.sudo),
            dry,
            log: Vec::new(),
        }
    }

    fn host(&self) -> String {
        self.shell.host.clone()
    }

    fn read(&mut self, script: &str) -> Result<String> {
        if self.dry {
            self.trace(script);
            return Ok(String::new());
        }
        self.shell.run(script)
    }

    fn run(&mut self, script: &str) -> Result<()> {
        if self.dry {
            self.trace(script);
            return Ok(());
        }
        self.shell.run(script).map(drop)
    }

    fn probe(&mut self, script: &str) -> Result<bool> {
        if self.dry {
            self.trace(script);
            return Ok(true);
        }
        self.shell.probe(script)
    }

    fn feed(&mut self, script: &str, input: &str) -> Result<()> {
        if self.dry {
            self.trace(script);
            return Ok(());
        }
        self.shell.run_with_input(script, input)
    }

    fn upload(&mut self, parent: &Path, name: &str, remote_dir: &str) -> Result<()> {
        if self.dry {
            self.trace(&crate::ssh::unpack_script(remote_dir));
            return Ok(());
        }
        self.shell.upload(parent, name, remote_dir)
    }

    fn trace(&mut self, script: &str) {
        let line = format!("ssh {} {script}", self.shell.host);
        println!("    {line}");
        self.log.push(line);
    }
}

pub fn deploy(config: &Config, opts: &Opts) -> Result<()> {
    let hosts = selected(config, opts)?;
    build(config, opts)?;
    let id = release::new_id();
    let unit_text = unit::render(config);
    for host in hosts {
        let mut runner = Runner::new(config, &host, opts.dry_run);
        deploy_host(&mut runner, config, &id, &unit_text)
            .with_context(|| format!("deploy of {id} failed on {host}"))?;
    }
    ui::ok(&format!("deployed {id}"));
    Ok(())
}

pub fn rollback(config: &Config, opts: &Opts) -> Result<()> {
    for host in selected(config, opts)? {
        let mut runner = Runner::new(config, &host, opts.dry_run);
        rollback_host(&mut runner, config).with_context(|| format!("rollback failed on {host}"))?;
    }
    Ok(())
}

pub fn status(config: &Config, opts: &Opts) -> Result<()> {
    for host in selected(config, opts)? {
        let mut runner = Runner::new(config, &host, false);
        status_host(&mut runner, config).with_context(|| format!("status failed on {host}"))?;
    }
    Ok(())
}

fn selected(config: &Config, opts: &Opts) -> Result<Vec<String>> {
    let hosts: Vec<String> = match &opts.host_filter {
        Some(filter) => config
            .hosts
            .iter()
            .filter(|host| host.contains(filter.as_str()))
            .cloned()
            .collect(),
        None => config.hosts.clone(),
    };
    if hosts.is_empty() {
        bail!("no configured host matches the --host filter");
    }
    Ok(hosts)
}

fn build(config: &Config, opts: &Opts) -> Result<()> {
    if let Some(command) = &config.build.command {
        if opts.skip_build {
            ui::note("build: skipped");
        } else {
            ui::note(&format!("build: {command}"));
            if !opts.dry_run {
                let status = std::process::Command::new("sh")
                    .args(["-c", command])
                    .status()
                    .context("cannot spawn sh")?;
                if !status.success() {
                    bail!("build command failed: {command}");
                }
            }
        }
    }
    if !opts.dry_run && !Path::new(&config.build.artifact).exists() {
        bail!("artifact {} does not exist", config.build.artifact);
    }
    Ok(())
}

fn deploy_host(runner: &mut Runner, config: &Config, id: &str, unit_text: &str) -> Result<()> {
    let host = runner.host();
    let root = config.root();
    ui::step(&host, &format!("release {id}"));
    let (parent, name) = artifact_parts(&config.build.artifact)?;
    runner.upload(parent, name, &format!("{root}/releases/{id}"))?;
    sync_unit(runner, config, unit_text)?;
    let previous = current_release(runner, &root)?;
    ui::step(&host, "flip current and restart");
    flip(runner, &root, id)?;
    restart(runner, config)?;
    if let Err(err) = check_health(runner, config) {
        return recover(runner, config, previous, err);
    }
    prune(runner, config, &root, id)?;
    ui::step(&host, "done");
    Ok(())
}

fn rollback_host(runner: &mut Runner, config: &Config) -> Result<()> {
    let host = runner.host();
    let root = config.root();
    let current = current_release(runner, &root)?;
    let ids = list_releases(runner, &root)?;
    let target = current.as_ref().and_then(|cur| {
        ids.iter()
            .rev()
            .find(|id| id.as_str() < cur.as_str())
            .cloned()
    });
    let Some(target) = target else {
        if runner.dry {
            ui::step(&host, "would flip to the release preceding current");
            return Ok(());
        }
        bail!("no release older than the current one to roll back to");
    };
    ui::step(
        &host,
        &format!(
            "rollback {} -> {target}",
            current.as_deref().unwrap_or("none")
        ),
    );
    flip(runner, &root, &target)?;
    restart(runner, config)?;
    check_health(runner, config)?;
    ui::step(&host, "done");
    Ok(())
}

fn status_host(runner: &mut Runner, config: &Config) -> Result<()> {
    let host = runner.host();
    let root = config.root();
    let current = current_release(runner, &root)?;
    let active = runner.read(&format!(
        "systemctl is-active {} 2>/dev/null || true",
        config.unit_name()
    ))?;
    let ids = list_releases(runner, &root)?;
    let recent: Vec<String> = ids
        .iter()
        .rev()
        .take(5)
        .map(|id| {
            if Some(id.as_str()) == current.as_deref() {
                format!("{id}*")
            } else {
                id.clone()
            }
        })
        .collect();
    ui::note(&format!(
        "{host}: {} {active}",
        current.as_deref().unwrap_or("none")
    ));
    ui::note(&format!("  releases: {}", recent.join(" ")));
    Ok(())
}

fn artifact_parts(artifact: &str) -> Result<(&Path, &str)> {
    let path = Path::new(artifact);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("build.artifact has no file name")?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Ok((parent, name))
}

fn sync_unit(runner: &mut Runner, config: &Config, rendered: &str) -> Result<()> {
    let host = runner.host();
    let unit_name = config.unit_name();
    let path = format!("/etc/systemd/system/{unit_name}");
    let existing = runner.read(&format!("cat {path} 2>/dev/null || true"))?;
    if existing == rendered.trim_end() {
        return Ok(());
    }
    ui::step(&host, "install unit");
    let sudo = runner.shell.sudo_prefix();
    let tmp = format!("/tmp/slipway-{unit_name}.tmp");
    runner.feed(&format!("cat > {tmp}"), rendered)?;
    runner.run(&format!(
        "{sudo}install -m 0644 {tmp} {path} && rm -f {tmp} \
         && {sudo}systemctl daemon-reload && {sudo}systemctl enable {unit_name}"
    ))?;
    Ok(())
}

fn current_release(runner: &mut Runner, root: &str) -> Result<Option<String>> {
    let target = runner.read(&format!("readlink {root}/current 2>/dev/null || true"))?;
    Ok(target
        .rsplit('/')
        .next()
        .filter(|name| release::is_id(name))
        .map(String::from))
}

fn list_releases(runner: &mut Runner, root: &str) -> Result<Vec<String>> {
    let listing = runner.read(&format!("ls -1 {root}/releases 2>/dev/null || true"))?;
    let mut ids: Vec<String> = listing
        .lines()
        .filter(|name| release::is_id(name))
        .map(str::to_string)
        .collect();
    ids.sort();
    Ok(ids)
}

fn flip(runner: &mut Runner, root: &str, id: &str) -> Result<()> {
    runner.run(&format!(
        "ln -sfn releases/{id} {root}/.current.tmp && mv -Tf {root}/.current.tmp {root}/current"
    ))
}

fn prune(runner: &mut Runner, config: &Config, root: &str, current: &str) -> Result<()> {
    let ids = list_releases(runner, root)?;
    let victims = release::prune_candidates(&ids, config.remote.keep_releases, Some(current));
    if victims.is_empty() {
        return Ok(());
    }
    ui::step(&runner.host(), &format!("prune {}", victims.join(" ")));
    runner.run(&format!(
        "cd {root}/releases && rm -rf -- {}",
        victims.join(" ")
    ))
}

fn restart(runner: &mut Runner, config: &Config) -> Result<()> {
    let sudo = runner.shell.sudo_prefix();
    runner.run(&format!("{sudo}systemctl restart {}", config.unit_name()))
}

fn check_health(runner: &mut Runner, config: &Config) -> Result<()> {
    let host = runner.host();
    let Some(hc) = &config.healthcheck else {
        ui::step(&host, "no healthcheck configured");
        return Ok(());
    };
    ui::step(&host, "healthcheck");
    for attempt in 1..=hc.retries {
        if runner.probe(&hc.command)? {
            return Ok(());
        }
        if attempt < hc.retries {
            thread::sleep(Duration::from_millis(hc.delay_ms));
        }
    }
    bail!(
        "health check `{}` did not pass after {} attempts",
        hc.command,
        hc.retries
    )
}

/// Runs after a failed health check. Always returns an error: the deploy
/// has failed either way, the question is only what state the host is in.
fn recover(
    runner: &mut Runner,
    config: &Config,
    previous: Option<String>,
    err: anyhow::Error,
) -> Result<()> {
    let host = runner.host();
    let Some(previous) = previous else {
        bail!("{err:#}; no previous release to roll back to");
    };
    ui::fail(&format!("  {host} unhealthy, rolling back to {previous}"));
    flip(runner, &config.root(), &previous)?;
    restart(runner, config)?;
    match check_health(runner, config) {
        Ok(()) => bail!("{err:#}; rolled back to {previous}, which is healthy again"),
        Err(second) => bail!("{err:#}; rolled back to {previous}, but: {second:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        toml::from_str(
            r#"
            app = "demo"
            hosts = ["deploy@h1"]
            [build]
            artifact = "out/demo"
            [service]
            exec_start = "{current}/demo"
            [healthcheck]
            command = "curl -fsS http://127.0.0.1:8080/health"
        "#,
        )
        .unwrap()
    }

    #[test]
    fn dry_run_plan_is_stable() {
        let config = test_config();
        let unit_text = unit::render(&config);
        let mut runner = Runner::new(&config, "deploy@h1", true);
        deploy_host(&mut runner, &config, "20260825120000", &unit_text).unwrap();
        let expected = [
            "ssh deploy@h1 mkdir -p /srv/demo/releases/20260825120000 \
             && tar -xzf - -C /srv/demo/releases/20260825120000",
            "ssh deploy@h1 cat /etc/systemd/system/demo.service 2>/dev/null || true",
            "ssh deploy@h1 cat > /tmp/slipway-demo.service.tmp",
            "ssh deploy@h1 sudo -n install -m 0644 /tmp/slipway-demo.service.tmp \
             /etc/systemd/system/demo.service && rm -f /tmp/slipway-demo.service.tmp \
             && sudo -n systemctl daemon-reload && sudo -n systemctl enable demo.service",
            "ssh deploy@h1 readlink /srv/demo/current 2>/dev/null || true",
            "ssh deploy@h1 ln -sfn releases/20260825120000 /srv/demo/.current.tmp \
             && mv -Tf /srv/demo/.current.tmp /srv/demo/current",
            "ssh deploy@h1 sudo -n systemctl restart demo.service",
            "ssh deploy@h1 curl -fsS http://127.0.0.1:8080/health",
            "ssh deploy@h1 ls -1 /srv/demo/releases 2>/dev/null || true",
        ];
        assert_eq!(runner.log, expected);
    }

    #[test]
    fn host_filter_selects_substrings_and_rejects_misses() {
        let mut config = test_config();
        config.hosts = vec![
            "deploy@app1".into(),
            "deploy@app2".into(),
            "deploy@db1".into(),
        ];
        let opts = |filter: Option<&str>| Opts {
            dry_run: true,
            skip_build: true,
            host_filter: filter.map(str::to_string),
        };
        assert_eq!(selected(&config, &opts(Some("app"))).unwrap().len(), 2);
        assert_eq!(selected(&config, &opts(None)).unwrap().len(), 3);
        assert!(selected(&config, &opts(Some("web"))).is_err());
    }

    #[test]
    fn artifact_parts_split_files_and_bare_names() {
        let (parent, name) = artifact_parts("target/release/demo").unwrap();
        assert_eq!((parent, name), (Path::new("target/release"), "demo"));
        let (parent, name) = artifact_parts("demo").unwrap();
        assert_eq!((parent, name), (Path::new("."), "demo"));
    }
}
