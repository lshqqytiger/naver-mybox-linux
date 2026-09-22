use anyhow::Result;

pub fn mount(mountpoint: &str) -> Result<()> {
    tracing::info!(mountpoint, "mount requested");
    Ok(())
}

pub fn unmount(mountpoint: &str) -> Result<()> {
    tracing::info!(mountpoint, "unmount requested");
    Ok(())
}
