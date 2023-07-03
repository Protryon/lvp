use std::{path::Path, process::Stdio};

use anyhow::{bail, Result};
use log::info;
use tokio::process::Command;

pub async fn run(command: &[&str]) -> Result<String> {
    let command = command.into_iter().map(|x| x.trim()).collect::<Vec<_>>();
    info!("running {}", command.join(" "));
    let out = Command::new(command[0])
        .args(&command[1..])
        .stdout(Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    if !out.status.success() {
        bail!(
            "{} exited with code: {}",
            command[0],
            out.status.code().unwrap_or_default()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

const CHROOT_BINDS: &[&str] = &[
    "lib", "lib64", "bin", "sbin", "proc", "dev", "sbin", "usr", "host", "var", "etc",
];
const CHROOT_BASE: &str = "/chr";

pub async fn create() -> Result<()> {
    let path = Path::new(CHROOT_BASE);
    tokio::fs::create_dir_all(path).await?;

    for bind in CHROOT_BINDS.into_iter().copied() {
        let out = path.join(bind);
        let from = Path::new("/").join(bind);
        tokio::fs::create_dir_all(&out).await?;
        run(&[
            "mount",
            "--rbind",
            from.to_str().unwrap(),
            out.to_str().unwrap(),
        ])
        .await?;
    }
    run(&[
        "mount",
        "-t",
        "devtmpfs",
        "none",
        path.join("dev").to_str().unwrap(),
    ])
    .await?;

    Ok(())
}

pub async fn run_in_chroot(command: &[&str]) -> Result<String> {
    let mut to_run: Vec<&str> = vec!["chroot", CHROOT_BASE];
    to_run.extend(command);
    Ok(run(&to_run).await?)
}
