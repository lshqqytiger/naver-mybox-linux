fn main() -> myboxfs::Result<()> {
    let result = myboxfs::mount_helper::run();
    if let Err(error) = &result {
        tracing::error!(%error, "MYBOX mount helper failed");
    }
    result
}
