//! Remote execution through the user's own ssh client.
//!
//! Every interaction is one `ssh` exec in BatchMode: aliases, jump hosts,
//! agents, and multiplexing from `~/.ssh/config` all apply, and nothing
//! ever prompts. Scripts run through the remote login shell; the config
//! validation in [`crate::config`] keeps interpolated values inert.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub struct Shell {
    pub host: String,
    pub sudo: bool,
}

impl Shell {
    pub fn new(host: &str, sudo: bool) -> Self {
        Self {
            host: host.to_string(),
            sudo,
        }
    }

    /// Prefix for remote commands that need root: unit install, systemctl
    /// mutations. `-n` keeps the no-password contract explicit.
    pub fn sudo_prefix(&self) -> &'static str {
        if self.sudo { "sudo -n " } else { "" }
    }

    pub fn ssh_args(&self, script: &str) -> Vec<String> {
        vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            "--".into(),
            self.host.clone(),
            script.into(),
        ]
    }

    /// Runs the script, expecting success; returns trimmed stdout.
    pub fn run(&self, script: &str) -> Result<String> {
        let output = Command::new("ssh")
            .args(self.ssh_args(script))
            .stdin(Stdio::null())
            .output()
            .context("cannot spawn ssh")?;
        if !output.status.success() {
            bail!("`{script}` failed: {}", stderr_head(&output.stderr));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Runs the script and reports only whether it succeeded.
    pub fn probe(&self, script: &str) -> Result<bool> {
        let status = Command::new("ssh")
            .args(self.ssh_args(script))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("cannot spawn ssh")?;
        Ok(status.success())
    }

    /// Runs the script with stdio inherited, streaming straight to the
    /// terminal; returns the remote exit code (None when killed by a
    /// signal, e.g. Ctrl-C on a followed journal).
    pub fn stream(&self, script: &str) -> Result<Option<i32>> {
        let status = Command::new("ssh")
            .args(self.ssh_args(script))
            .status()
            .context("cannot spawn ssh")?;
        Ok(status.code())
    }

    /// Runs the script with `input` on its stdin.
    pub fn run_with_input(&self, script: &str, input: &str) -> Result<()> {
        let mut child = Command::new("ssh")
            .args(self.ssh_args(script))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .context("cannot spawn ssh")?;
        child
            .stdin
            .take()
            .expect("stdin was requested piped")
            .write_all(input.as_bytes())
            .context("cannot stream input to ssh")?;
        let status = child.wait().context("cannot wait for ssh")?;
        if !status.success() {
            bail!("`{script}` failed");
        }
        Ok(())
    }

    /// Streams `{parent}/{name}` as a tarball into `remote_dir` in one ssh
    /// exec, so files and directories upload the same way.
    pub fn upload(&self, parent: &Path, name: &str, remote_dir: &str) -> Result<()> {
        let mut tar = Command::new("tar")
            .arg("-C")
            .arg(parent)
            .args(["-czf", "-", "--", name])
            .stdout(Stdio::piped())
            .spawn()
            .context("cannot spawn tar")?;
        let stream = tar.stdout.take().expect("stdout was requested piped");
        let status = Command::new("ssh")
            .args(self.ssh_args(&unpack_script(remote_dir)))
            .stdin(Stdio::from(stream))
            .stdout(Stdio::null())
            .status()
            .context("cannot spawn ssh")?;
        let tar_status = tar.wait().context("cannot wait for tar")?;
        if !tar_status.success() {
            bail!("tar failed for {name:?} in {}", parent.display());
        }
        if !status.success() {
            bail!("upload into {remote_dir} failed");
        }
        Ok(())
    }
}

pub fn unpack_script(remote_dir: &str) -> String {
    format!("mkdir -p {remote_dir} && tar -xzf - -C {remote_dir}")
}

fn stderr_head(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().take(3).collect();
    if lines.is_empty() {
        "no stderr".to_string()
    } else {
        lines.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_args_pin_batch_mode_and_separate_options_from_the_host() {
        let shell = Shell::new("deploy@h1", true);
        assert_eq!(
            shell.ssh_args("echo hi"),
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                "deploy@h1",
                "echo hi"
            ]
        );
    }

    #[test]
    fn sudo_prefix_follows_config() {
        assert_eq!(Shell::new("h", true).sudo_prefix(), "sudo -n ");
        assert_eq!(Shell::new("h", false).sudo_prefix(), "");
    }

    #[test]
    fn unpack_script_creates_the_target_first() {
        assert_eq!(
            unpack_script("/srv/demo/releases/20260101000000"),
            "mkdir -p /srv/demo/releases/20260101000000 \
             && tar -xzf - -C /srv/demo/releases/20260101000000"
        );
    }
}
