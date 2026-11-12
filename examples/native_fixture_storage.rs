//! CI fixture tooling only. This executable is never included in a consumer
//! package or exposed through the installation's control API.
use amplifier_native_platform::{private_create_dir, validate_private_directory};
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().context("Choose one fixture directory")?);
    ensure!(args.next().is_none(), "Choose only one fixture directory");
    ensure!(path.is_absolute(), "Fixture directory must be absolute");
    match private_create_dir(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    // Existing permissions/ownership are never repaired by this fixture.
    validate_private_directory(&path)?;
    Ok(())
}
