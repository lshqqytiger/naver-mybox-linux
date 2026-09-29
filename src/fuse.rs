use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime},
};

use fuser::{
    Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo,
    MountOption, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, ReplyOpen, Request,
};

use crate::{
    Result,
    api::{MyboxApiClient, RemoteDrive, RemoteEntry},
};

const ROOT_INODE: u64 = 1;
const ATTR_TTL: Duration = Duration::from_secs(1);

struct Node {
    inode: u64,
    parent: u64,
    entry: RemoteEntry,
}

struct NodeTable {
    nodes: HashMap<u64, Node>,
    children: HashMap<u64, Vec<u64>>,
    loaded_directories: HashSet<u64>,
    next_inode: u64,
}

impl NodeTable {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            children: HashMap::new(),
            loaded_directories: HashSet::new(),
            next_inode: ROOT_INODE + 1,
        }
    }
}

pub struct MyboxFs<D> {
    drive: D,
    nodes: Mutex<NodeTable>,
    uid: u32,
    gid: u32,
}

impl<D: RemoteDrive> MyboxFs<D> {
    pub fn new(drive: D) -> Self {
        Self {
            drive,
            nodes: Mutex::new(NodeTable::new()),
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
        }
    }

    fn ensure_children(&self, parent: u64) -> Result<()> {
        let folder_id = {
            let nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
            if nodes.loaded_directories.contains(&parent) {
                return Ok(());
            }
            if parent == ROOT_INODE {
                None
            } else {
                let node = nodes
                    .nodes
                    .get(&parent)
                    .ok_or("directory inode not found")?;
                if !node.entry.is_directory() {
                    return Err("inode is not a directory".into());
                }
                Some(node.entry.resource_id.clone())
            }
        };

        let entries = match folder_id {
            Some(folder_id) => self.drive.list_children(&folder_id)?,
            None => self.drive.list_root()?,
        };

        let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        if nodes.loaded_directories.contains(&parent) {
            return Ok(());
        }

        let child_inodes = entries
            .into_iter()
            .map(|entry| {
                let inode = nodes.next_inode;
                nodes.next_inode += 1;
                nodes.nodes.insert(
                    inode,
                    Node {
                        inode,
                        parent,
                        entry,
                    },
                );
                inode
            })
            .collect();
        nodes.children.insert(parent, child_inodes);
        nodes.loaded_directories.insert(parent);
        Ok(())
    }

    fn entry_attr(&self, node: &Node) -> FileAttr {
        let kind = if node.entry.is_directory() {
            FileType::Directory
        } else {
            FileType::RegularFile
        };
        let size = if node.entry.is_directory() {
            0
        } else {
            node.entry.size
        };

        FileAttr {
            ino: INodeNo(node.inode),
            size,
            blocks: size.div_ceil(512),
            atime: SystemTime::UNIX_EPOCH,
            mtime: SystemTime::UNIX_EPOCH,
            ctime: SystemTime::UNIX_EPOCH,
            crtime: SystemTime::UNIX_EPOCH,
            kind,
            perm: if node.entry.is_directory() {
                0o555
            } else {
                0o444
            },
            nlink: if node.entry.is_directory() { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn root_attr(&self) -> FileAttr {
        FileAttr {
            ino: INodeNo(ROOT_INODE),
            size: 0,
            blocks: 0,
            atime: SystemTime::UNIX_EPOCH,
            mtime: SystemTime::UNIX_EPOCH,
            ctime: SystemTime::UNIX_EPOCH,
            crtime: SystemTime::UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o555,
            nlink: 2,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn lookup_entry(&self, parent: u64, name: &OsStr) -> Result<Option<FileAttr>> {
        self.ensure_children(parent)?;
        let nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        let Some(children) = nodes.children.get(&parent) else {
            return Ok(None);
        };
        Ok(children.iter().find_map(|inode| {
            let node = nodes.nodes.get(inode)?;
            (node.entry.name == name.to_string_lossy()).then(|| self.entry_attr(node))
        }))
    }

    fn attr(&self, inode: u64) -> Option<FileAttr> {
        if inode == ROOT_INODE {
            return Some(self.root_attr());
        }
        let nodes = self.nodes.lock().ok()?;
        nodes.nodes.get(&inode).map(|node| self.entry_attr(node))
    }

    fn directory_entries(&self, inode: u64) -> Result<Vec<(u64, FileType, String)>> {
        self.ensure_children(inode)?;
        let nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        let children = nodes
            .children
            .get(&inode)
            .ok_or("directory inode not found")?;
        Ok(children
            .iter()
            .filter_map(|child| nodes.nodes.get(child))
            .map(|node| {
                (
                    node.inode,
                    if node.entry.is_directory() {
                        FileType::Directory
                    } else {
                        FileType::RegularFile
                    },
                    node.entry.name.clone(),
                )
            })
            .collect())
    }

    fn read_file(&self, inode: u64, offset: u64, size: u32) -> Result<Vec<u8>> {
        let (file_id, file_size) = {
            let nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
            let node = nodes.nodes.get(&inode).ok_or("file inode not found")?;
            if node.entry.is_directory() {
                return Err("cannot read a directory".into());
            }
            (node.entry.resource_id.clone(), node.entry.size)
        };
        let size = (file_size.saturating_sub(offset)).min(size as u64) as u32;
        self.drive.download_file(&file_id, offset, size)
    }
}

impl<D: RemoteDrive> Filesystem for MyboxFs<D> {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.lookup_entry(parent.0, name) {
            Ok(Some(attr)) => reply.entry(&ATTR_TTL, &attr, Generation(0)),
            Ok(None) => reply.error(Errno::ENOENT),
            Err(error) => {
                tracing::warn!(%error, "MYBOX lookup failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.attr(ino.0) {
            Some(attr) => reply.attr(&ATTR_TTL, &attr),
            None => reply.error(Errno::ENOENT),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        match self.attr(ino.0) {
            Some(attr) if attr.kind == FileType::RegularFile => {
                reply.opened(FileHandle(0), FopenFlags::empty())
            }
            Some(_) => reply.error(Errno::EISDIR),
            None => reply.error(Errno::ENOENT),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        match self.directory_entries(ino.0) {
            Ok(entries) => {
                let mut all_entries = vec![
                    (ROOT_INODE, FileType::Directory, ".".to_owned()),
                    (
                        if ino.0 == ROOT_INODE {
                            ROOT_INODE
                        } else {
                            self.nodes
                                .lock()
                                .ok()
                                .and_then(|nodes| nodes.nodes.get(&ino.0).map(|node| node.parent))
                                .unwrap_or(ROOT_INODE)
                        },
                        FileType::Directory,
                        "..".to_owned(),
                    ),
                ];
                all_entries.extend(entries);
                for (index, (inode, kind, name)) in
                    all_entries.into_iter().enumerate().skip(offset as usize)
                {
                    if reply.add(INodeNo(inode), (index + 1) as u64, kind, name) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(error) => {
                tracing::warn!(%error, "MYBOX directory listing failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        match self.read_file(ino.0, offset, size) {
            Ok(bytes) => reply.data(&bytes),
            Err(error) => {
                tracing::warn!(%error, "MYBOX file read failed");
                reply.error(Errno::EIO);
            }
        }
    }
}

pub fn mount(mountpoint: &Path, access_token: String) -> Result<()> {
    let filesystem = MyboxFs::new(MyboxApiClient::new(access_token));
    let mut config = Config::default();
    config.mount_options = vec![MountOption::RO, MountOption::FSName("myboxfs".into())];
    fuser::mount(filesystem, mountpoint, &config)?;
    Ok(())
}

pub fn unmount(mountpoint: &Path) -> Result<()> {
    for command in ["fusermount3", "fusermount"] {
        match std::process::Command::new(command)
            .args(["-u", "--", &mountpoint.to_string_lossy()])
            .status()
        {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) | Err(_) => continue,
        }
    }
    Err(format!(
        "failed to unmount {}; neither fusermount3 nor fusermount succeeded",
        mountpoint.display()
    )
    .into())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeDrive {
        root: Vec<RemoteEntry>,
        children: HashMap<String, Vec<RemoteEntry>>,
        files: HashMap<String, Vec<u8>>,
        downloads: Mutex<Vec<(String, u64, u32)>>,
    }

    impl RemoteDrive for FakeDrive {
        fn list_root(&self) -> Result<Vec<RemoteEntry>> {
            Ok(self.root.clone())
        }
        fn list_children(&self, folder_id: &str) -> Result<Vec<RemoteEntry>> {
            Ok(self.children.get(folder_id).cloned().unwrap_or_default())
        }
        fn download_file(&self, file_id: &str, offset: u64, size: u32) -> Result<Vec<u8>> {
            self.downloads
                .lock()
                .unwrap()
                .push((file_id.to_owned(), offset, size));
            let file = self.files.get(file_id).ok_or("missing test file")?;
            let start = (offset as usize).min(file.len());
            let end = start.saturating_add(size as usize).min(file.len());
            Ok(file[start..end].to_vec())
        }
    }

    fn entry(id: &str, name: &str, size: u64, kind: &str) -> RemoteEntry {
        RemoteEntry {
            resource_id: id.into(),
            name: name.into(),
            size,
            kind: kind.into(),
        }
    }

    fn filesystem() -> MyboxFs<FakeDrive> {
        MyboxFs::new(FakeDrive {
            root: vec![
                entry("folder-1", "Documents", 0, "folder"),
                entry("file-1", "hello.txt", 11, "file"),
            ],
            children: HashMap::from([(
                "folder-1".into(),
                vec![entry("file-2", "notes.txt", 5, "file")],
            )]),
            files: HashMap::from([
                ("file-1".into(), b"hello world".to_vec()),
                ("file-2".into(), b"notes".to_vec()),
            ]),
            downloads: Mutex::new(Vec::new()),
        })
    }

    #[test]
    fn browses_directories_and_returns_file_attributes() {
        let filesystem = filesystem();
        let documents = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("Documents"))
            .unwrap()
            .unwrap();
        let hello = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();

        assert_eq!(documents.kind, FileType::Directory);
        assert_eq!(documents.perm, 0o555);
        assert_eq!(hello.kind, FileType::RegularFile);
        assert_eq!(hello.size, 11);
        assert_eq!(hello.perm, 0o444);

        let children = filesystem.directory_entries(documents.ino.0).unwrap();
        assert_eq!(
            children,
            vec![(4, FileType::RegularFile, "notes.txt".into())]
        );
    }

    #[test]
    fn reads_a_requested_file_range() {
        let filesystem = filesystem();
        let hello = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();

        assert_eq!(filesystem.read_file(hello.ino.0, 6, 3).unwrap(), b"wor");
        assert_eq!(filesystem.read_file(hello.ino.0, 10, 8).unwrap(), b"d");
        assert!(filesystem.read_file(hello.ino.0, 11, 8).unwrap().is_empty());
        assert_eq!(
            *filesystem.drive.downloads.lock().unwrap(),
            vec![
                ("file-1".into(), 6, 3),
                ("file-1".into(), 10, 1),
                ("file-1".into(), 11, 0)
            ]
        );
    }
}
