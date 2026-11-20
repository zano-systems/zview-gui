use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Domains {
    default: String,
    build_dir: PathBuf,
    domains: Vec<Domain>,
}
#[derive(Deserialize)]
struct Domain {
    name: String,
    build_dir: PathBuf,
}
#[derive(Default, Deserialize)]
struct Runners {
    #[serde(default)]
    config: Config,
    #[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
    #[serde(default)]
    args: RunnerArgs,
}
#[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
#[derive(Default, Deserialize)]
struct RunnerArgs {
    #[serde(default)]
    jlink: Vec<String>,
}
#[derive(Default, Deserialize)]
struct Config {
    elf_file: Option<PathBuf>,
}

fn runners(build: &Path, requested: Option<&str>) -> Result<(PathBuf, Runners)> {
    let build = std::path::absolute(build)?;
    let domains = build.join("domains.yaml");
    let image = if domains.is_file() {
        let data: Domains = serde_yaml_ng::from_reader(File::open(&domains)?)
            .with_context(|| format!("parse {}", domains.display()))?;
        let name = requested.unwrap_or(&data.default);
        let domain = data
            .domains
            .iter()
            .find(|d| d.name == name)
            .with_context(|| format!("unknown build domain {name}"))?;
        match domain.build_dir.strip_prefix(&data.build_dir) {
            Ok(relative) => build.join(relative),
            Err(_) => domain.build_dir.clone(),
        }
    } else {
        if requested.is_some_and(|d| d != "app") {
            bail!("build has only the app domain")
        }
        build
    };
    let zephyr = image.join("zephyr");
    let runners = zephyr.join("runners.yaml");
    let config: Runners = if runners.is_file() {
        serde_yaml_ng::from_reader(File::open(&runners)?)
            .with_context(|| format!("parse {}", runners.display()))?
    } else {
        Default::default()
    };
    Ok((zephyr, config))
}

pub fn elf(build: &Path, requested: Option<&str>) -> Result<PathBuf> {
    let (zephyr, config) = runners(build, requested)?;
    Ok(zephyr.join(
        config
            .config
            .elf_file
            .unwrap_or_else(|| "zephyr.elf".into()),
    ))
}

#[cfg(all(feature = "probe-rs", not(target_family = "wasm")))]
pub fn jlink_chip(build: &Path, requested: Option<&str>) -> Result<String> {
    let (zephyr, config) = runners(build, requested)?;
    let mut args = config.args.jlink.iter().map(String::as_str);
    let mut chip = None;
    while let Some(arg) = args.next() {
        let value = if arg == "--device" {
            args.next()
        } else if let Some(value) = arg.strip_prefix("--device=") {
            Some(value)
        } else {
            continue;
        };
        let value = value
            .filter(|v| !v.is_empty() && !v.starts_with('-'))
            .context("missing J-Link --device value; supply --chip")?;
        if chip.is_some_and(|previous: &str| !previous.eq_ignore_ascii_case(value)) {
            bail!("conflicting J-Link --device values; supply --chip");
        }
        chip = Some(value);
    }
    chip.map(str::to_owned).with_context(|| {
        format!(
            "no J-Link --device in {}; supply --chip",
            zephyr.join("runners.yaml").display()
        )
    })
}
