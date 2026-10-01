fn main() -> myboxfs::Result<()> {
    myboxfs::logging::init(false)?;
    myboxfs::cli::run()
}
