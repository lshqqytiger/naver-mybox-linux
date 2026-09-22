use std::{
    env,
    fs::{self, OpenOptions},
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
        fs::create_dir_all(parent)?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.config_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }

        file.write_all(token.trim().as_bytes())?;
        file.write_all(b"\n")?;
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
    fn rejects_an_empty_token() {
        let store = TokenStore::new(test_path());
        assert!(store.save(" \n").is_err());
    }
}
