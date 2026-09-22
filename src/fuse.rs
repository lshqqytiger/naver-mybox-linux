use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::Path,
    time::{Duration, SystemTime},
};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyDirectory, ReplyEntry, Request,
};

use crate::{
    Result,
    api::{DirectoryEntry, EntryKind, MyboxApiClient},
};

const ROOT_INODE: u64 = 1;
const ATTRIBUTE_TTL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryChild {
    pub inode: u64,
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Clone)]
struct Node {
    entry: DirectoryEntry,
    parent: u64,
}

pub struct MyboxFilesystem {
    nodes: BTreeMap<u64, Node>,
}

impl MyboxFilesystem {
    pub fn new(client: &MyboxApiClient) -> Result<Self> {
        let root_children = client.list_children("/")?;
        let mut filesystem = Self {
            nodes: BTreeMap::from([(
                ROOT_INODE,
                Node {
                    entry: DirectoryEntry::directory("", root_children),
                    parent: ROOT_INODE,
                },
            )]),
        };
        filesystem.add_children(ROOT_INODE)?;
        Ok(filesystem)
    }

    pub fn read_dir(&self, inode: u64) -> Result<Vec<DirectoryChild>> {
        let node = self.node(inode)?;
        if node.entry.kind != EntryKind::Directory {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "inode is not a directory",
            )
            .into());
        }

        let mut children = vec![
            DirectoryChild {
                inode,
                name: ".".to_owned(),
                kind: EntryKind::Directory,
            },
            DirectoryChild {
                inode: node.parent,
                name: "..".to_owned(),
                kind: EntryKind::Directory,
            },
        ];
        children.extend(
            self.nodes
                .iter()
                .filter(|(_, child)| child.parent == inode && child.entry.name != "")
                .map(|(inode, child)| DirectoryChild {
                    inode: *inode,
                    name: child.entry.name.clone(),
                    kind: child.entry.kind.clone(),
                }),
        );
        Ok(children)
    }

    fn add_children(&mut self, parent: u64) -> Result<()> {
        let children = self.node(parent)?.entry.children.clone();
        for entry in children {
            let inode = self
                .nodes
                .last_key_value()
                .map_or(ROOT_INODE + 1, |(inode, _)| inode + 1);
            self.nodes.insert(inode, Node { entry, parent });
            self.add_children(inode)?;
        }
        Ok(())
    }

    fn node(&self, inode: u64) -> Result<&Node> {
        self.nodes.get(&inode).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "MYBOX inode was not found").into()
        })
    }

    fn attr(&self, inode: u64) -> Result<FileAttr> {
        let node = self.node(inode)?;
        Ok(FileAttr {
            ino: inode,
            size: node.entry.size,
            blocks: node.entry.size.div_ceil(512),
            atime: SystemTime::UNIX_EPOCH,
            mtime: SystemTime::UNIX_EPOCH,
            ctime: SystemTime::UNIX_EPOCH,
            crtime: SystemTime::UNIX_EPOCH,
            kind: file_type(&node.entry.kind),
            perm: if node.entry.kind == EntryKind::Directory {
                0o555
            } else {
                0o444
            },
            nlink: if node.entry.kind == EntryKind::Directory {
                2
            } else {
                1
            },
            uid: 0,
            gid: 0,
            rdev: 0,
            flags: 0,
            blksize: 512,
        })
    }
}

impl Filesystem for MyboxFilesystem {
    fn lookup(&mut self, _request: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let Some(name) = name.to_str() else {
            reply.error(libc::ENOENT);
            return;
        };
        match self
            .read_dir(parent)
            .ok()
            .and_then(|children| children.into_iter().find(|child| child.name == name))
            .and_then(|child| self.attr(child.inode).ok())
        {
            Some(attr) => reply.entry(&ATTRIBUTE_TTL, &attr, 0),
            None => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _request: &Request<'_>, inode: u64, _fh: Option<u64>, reply: ReplyAttr) {
        match self.attr(inode) {
            Ok(attr) => reply.attr(&ATTRIBUTE_TTL, &attr),
            Err(_) => reply.error(libc::ENOENT),
        }
    }

    fn readdir(
        &mut self,
        _request: &Request<'_>,
        inode: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        match self.read_dir(inode) {
            Ok(children) => {
                for (index, child) in children.into_iter().enumerate().skip(offset as usize) {
                    if reply.add(
                        child.inode,
                        (index + 1) as i64,
                        file_type(&child.kind),
                        child.name,
                    ) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(_) => reply.error(libc::ENOENT),
        }
    }
}

pub fn mount(mountpoint: &Path) -> Result<()> {
    let client = MyboxApiClient::from_environment()?;
    mount_with_client(mountpoint, &client)
}

pub fn mount_with_client(mountpoint: &Path, client: &MyboxApiClient) -> Result<()> {
    let filesystem = MyboxFilesystem::new(client)?;
    tracing::info!(mountpoint = %mountpoint.display(), "mounting MYBOX filesystem");
    fuser::mount2(
        filesystem,
        mountpoint,
        &[MountOption::RO, MountOption::FSName("myboxfs".to_owned())],
    )
    .map_err(Into::into)
}

pub fn unmount(mountpoint: &Path) -> Result<()> {
    tracing::info!(mountpoint = %mountpoint.display(), "unmount requested");
    Ok(())
}

fn file_type(kind: &EntryKind) -> FileType {
    match kind {
        EntryKind::Directory => FileType::Directory,
        EntryKind::File => FileType::RegularFile,
    }
}

#[cfg(test)]
mod tests {
    use crate::api::{DirectoryEntry, EntryKind, MyboxApiClient};

    use super::MyboxFilesystem;

    #[test]
    fn read_dir_includes_dot_entries_and_children() {
        let client = MyboxApiClient::connect(
            "token",
            vec![
                DirectoryEntry::directory("photos", Vec::new()),
                DirectoryEntry::file("readme.txt", 12),
            ],
        )
        .unwrap();
        let filesystem = MyboxFilesystem::new(&client).unwrap();

        let children = filesystem.read_dir(1).unwrap();
        assert_eq!(children.len(), 4);
        assert_eq!(children[0].name, ".");
        assert_eq!(children[1].name, "..");
        assert_eq!(children[2].kind, EntryKind::Directory);
        assert_eq!(children[3].name, "readme.txt");
    }
}
