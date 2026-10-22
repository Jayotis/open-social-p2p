//! `osp service`: run the node as a systemd user service, so it starts at
//! boot without anyone logging in and restarts if it stops (ADR 0006).
//!
//! A user service needs no root access. Starting it at boot does need
//! "lingering" turned on for the user, which `install` asks logind for.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use osp_node::Home;
use tokio::net::UnixStream;

const UNIT: &str = "osp.service";

pub async fn install(home: &Path) -> Result<()> {
    let home = std::path::absolute(home)?;
    let node = Home::new(&home);
    ensure!(
        node.is_initialized(),
        "no node in {}; run `osp init` first",
        home.display()
    );
    if UnixStream::connect(node.socket()).await.is_ok() {
        bail!(
            "a node is already running in {}; stop it first (Ctrl-C in its terminal), then run this again",
            home.display()
        );
    }
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("finding the osp binary")?;

    let path = unit_path()?;
    std::fs::create_dir_all(path.parent().context("unit path has no parent")?)?;
    std::fs::write(&path, unit_file(&exe, &home)?)
        .with_context(|| format!("writing {}", path.display()))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", UNIT])?;
    println!("Installed {} and started the node.", path.display());
    println!("It runs {} --home {}", exe.display(), home.display());
    if exe.components().any(|c| c.as_os_str() == "target") {
        println!(
            "Note: that binary is inside a build directory. Copy it somewhere stable (e.g. ~/.local/bin) and install again."
        );
    }

    if !lingering() {
        // Without lingering, user services start only when the user logs in.
        let enabled = Command::new("loginctl")
            .arg("enable-linger")
            .status()
            .is_ok_and(|s| s.success());
        if enabled && lingering() {
            println!("Turned on lingering, so the node starts at boot without you logging in.");
        } else {
            println!("To start the node at boot without logging in, run:");
            println!("    sudo loginctl enable-linger {}", user()?);
        }
    }
    println!("Logs: journalctl --user -u osp -f");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let path = unit_path()?;
    if !path.exists() {
        bail!("no service installed ({} doesn't exist)", path.display());
    }
    systemctl(&["disable", "--now", UNIT])?;
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    systemctl(&["daemon-reload"])?;
    println!("Stopped and removed the service. The node's data is untouched.");
    println!("Start it by hand with `osp daemon`, or install the service again.");
    Ok(())
}

fn unit_file(exe: &Path, home: &Path) -> Result<String> {
    Ok(format!(
        "# Installed by `osp service install`; remove with `osp service uninstall`.
[Unit]
Description=Open Social P2P home node
Documentation=https://github.com/Jayotis/open-social-p2p

[Service]
ExecStart=\"{}\" --home \"{}\" daemon
Restart=on-failure
RestartSec=10
NoNewPrivileges=yes
# Interactive programs come first; transcoding and serving just take a little longer.
Nice=10
IOSchedulingClass=best-effort
IOSchedulingPriority=7

[Install]
WantedBy=default.target
",
        unit_quote(exe)?,
        unit_quote(home)?
    ))
}

/// A path for use inside double quotes in a unit file. `%` starts a
/// systemd specifier, so it is doubled; quotes and backslashes are refused.
fn unit_quote(path: &Path) -> Result<String> {
    let s = path
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
    ensure!(
        !s.contains(['"', '\\', '\n']),
        "{s} contains characters a unit file can't hold; use a simpler path"
    );
    Ok(s.replace('%', "%%"))
}

fn unit_path() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new().context("no home directory")?;
    Ok(dirs.config_dir().join("systemd/user").join(UNIT))
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("running systemctl (is this a systemd system?)")?;
    ensure!(
        status.success(),
        "systemctl --user {} failed",
        args.join(" ")
    );
    Ok(())
}

fn lingering() -> bool {
    user().is_ok_and(|u| Path::new("/var/lib/systemd/linger").join(u).exists())
}

fn user() -> Result<String> {
    std::env::var("USER").context("USER is not set")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_paths_are_quoted_safely() {
        let unit = unit_file(Path::new("/opt/osp/bin/osp"), Path::new("/home/a b/100%")).unwrap();
        assert!(unit.contains("ExecStart=\"/opt/osp/bin/osp\" --home \"/home/a b/100%%\" daemon"));
        assert!(unit_quote(Path::new("/home/x\"y")).is_err());
    }
}
