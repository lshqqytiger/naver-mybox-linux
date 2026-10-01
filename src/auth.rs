use std::{
    env,
    fs::{self, DirBuilder, File},
    io::Write,
    path::PathBuf,
};

use crate::Result;

pub struct TokenStore {
    pub config_path: PathBuf,
}

impl TokenStore {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }

    pub fn default_path() -> Result<PathBuf> {
        let config_dir = match env::var_os("XDG_CONFIG_HOME") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => PathBuf::from(
                env::var_os("HOME").ok_or("HOME is not set and XDG_CONFIG_HOME is unavailable")?,
            )
            .join(".config"),
        };

        Ok(config_dir.join("myboxfs").join("token"))
    }

    pub fn load(&self) -> Result<Option<String>> {
        match fs::read_to_string(&self.config_path) {
            Ok(token) => {
                let token = token.trim().to_owned();
                if token.is_empty() {
                    Err(format!("stored token at {} is empty", self.config_path.display()).into())
                } else {
                    Ok(Some(token))
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, token: &str) -> Result<()> {
        if token.trim().is_empty() {
            return Err("access token cannot be empty".into());
        }

        let parent = self
            .config_path
            .parent()
            .ok_or("token path must have a parent directory")?;
        let mut builder = DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
        let directory = fs::symlink_metadata(parent)?;
        if !directory.is_dir() {
            return Err("token directory must not be a symlink".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if directory.uid() != unsafe { libc::geteuid() } || directory.mode() & 0o077 != 0 {
                return Err(
                    "token directory must be owned by the current user with mode 0700".into(),
                );
            }
        }
        match fs::symlink_metadata(&self.config_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("refusing to replace a symlink token file".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(token.trim().as_bytes())?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.config_path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::TokenStore;

    fn test_path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("myboxfs-auth-test-{unique}"))
            .join("token")
    }

    #[test]
    fn saves_and_loads_a_token() {
        let path = test_path();
        let store = TokenStore::new(path.clone());

        assert_eq!(store.load().unwrap(), None);
        store.save("  mbx_pat_test  ").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("mbx_pat_test"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn atomically_replaces_token_without_changing_open_old_file() {
        use std::io::Read;
        let path = test_path();
        let store = TokenStore::new(path.clone());
        store.save("old-token").unwrap();
        let mut old = fs::File::open(&path).unwrap();
        store.save("new-token").unwrap();
        let mut bytes = String::new();
        old.read_to_string(&mut bytes).unwrap();
        assert_eq!(bytes, "old-token\n");
        assert_eq!(store.load().unwrap().as_deref(), Some("new-token"));
        assert!(store.save(" ").is_err());
        assert_eq!(store.load().unwrap().as_deref(), Some("new-token"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_and_preserves_failed_replacement() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let path = test_path();
        let store = TokenStore::new(path.clone());
        store.save("original").unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(
            fs::metadata(parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let target = parent.join("target");
        fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        assert!(store.save("replacement").is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "original\n");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("marker"), "preserved").unwrap();
        assert!(store.save("replacement").is_err());
        assert_eq!(
            fs::read_to_string(path.join("marker")).unwrap(),
            "preserved"
        );
        assert_eq!(fs::read_dir(parent).unwrap().count(), 2);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn rejects_an_empty_token() {
        let store = TokenStore::new(test_path());
        assert!(store.save(" \n").is_err());
    }
}
