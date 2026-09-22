use crate::Result;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub children: Vec<DirectoryEntry>,
}

impl DirectoryEntry {
    pub fn directory(name: impl Into<String>, children: Vec<Self>) -> Self {
        Self {
            name: name.into(),
            kind: EntryKind::Directory,
            size: 0,
            children,
        }
    }

    pub fn file(name: impl Into<String>, size: u64) -> Self {
        Self {
            name: name.into(),
            kind: EntryKind::File,
            size,
            children: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MyboxApiClient {
    access_token: Option<String>,
    root: DirectoryEntry,
}

impl MyboxApiClient {
    pub fn new() -> Self {
        Self {
            access_token: None,
            root: DirectoryEntry::directory("", Vec::new()),
        }
    }

    pub fn connect(
        access_token: impl Into<String>,
        root_children: Vec<DirectoryEntry>,
    ) -> Result<Self> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "MYBOX access token must not be empty",
            )
            .into());
        }

        validate_entries(&root_children)?;
        Ok(Self {
            access_token: Some(access_token),
            root: DirectoryEntry::directory("", root_children),
        })
    }

    pub fn from_environment() -> Result<Self> {
        let token = std::env::var("MYBOX_ACCESS_TOKEN").map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "MYBOX_ACCESS_TOKEN is not set",
            )
        })?;
        Self::connect(token, Vec::new())
    }

    pub fn health_check(&self) -> Result<()> {
        if self.access_token.is_some() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "MYBOX client is not connected",
            )
            .into())
        }
    }

    pub fn list_children(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        self.health_check()?;
        let mut entry = &self.root;
        for component in path
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
        {
            entry = entry
                .children
                .iter()
                .find(|child| child.name == component)
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "MYBOX path was not found")
                })?;
        }

        if entry.kind != EntryKind::Directory {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "MYBOX path is not a directory",
            )
            .into());
        }

        Ok(entry.children.clone())
    }
}

impl Default for MyboxApiClient {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_entries(entries: &[DirectoryEntry]) -> Result<()> {
    for entry in entries {
        if entry.name.is_empty()
            || entry.name == "."
            || entry.name == ".."
            || entry.name.contains('/')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "MYBOX entry names must be single path components",
            )
            .into());
        }
        if entry.kind == EntryKind::File && !entry.children.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "MYBOX files cannot have children",
            )
            .into());
        }
        validate_entries(&entry.children)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DirectoryEntry, MyboxApiClient};

    #[test]
    fn connects_and_lists_directory_children() {
        let client = MyboxApiClient::connect(
            "token",
            vec![DirectoryEntry::directory(
                "documents",
                vec![DirectoryEntry::file("notes.txt", 42)],
            )],
        )
        .unwrap();

        assert_eq!(client.list_children("/").unwrap().len(), 1);
        assert_eq!(
            client.list_children("/documents").unwrap()[0].name,
            "notes.txt"
        );
    }

    #[test]
    fn rejects_empty_access_token() {
        assert!(MyboxApiClient::connect("", Vec::new()).is_err());
    }
}
