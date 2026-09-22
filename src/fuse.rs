use std::path::Path;

use anyhow::Result;

pub fn mount(mountpoint: &Path) -> Result<()> {
    tracing::info!(mountpoint = %mountpoint.display(), "mount requested");
    Ok(())
}

pub fn unmount(mountpoint: &Path) -> Result<()> {
    tracing::info!(mountpoint = %mountpoint.display(), "unmount requested");
    Ok(())
}
