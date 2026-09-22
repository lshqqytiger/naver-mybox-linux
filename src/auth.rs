use std::path::PathBuf;

pub struct TokenStore {
    pub config_path: PathBuf,
}

impl TokenStore {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}
