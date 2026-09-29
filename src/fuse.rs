use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime},
};

use fuser::{
    Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo,
    MountOption, OpenFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request,
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
    dirty: HashMap<u64, Vec<u8>>,
    baselines: HashMap<u64, RemoteEntry>,
    open_files: HashMap<u64, u64>,
    lookups: HashMap<u64, u64>,
    unlinked: HashSet<u64>,
    loaded_directories: HashSet<u64>,
    next_inode: u64,
}

impl NodeTable {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            children: HashMap::new(),
            dirty: HashMap::new(),
            baselines: HashMap::new(),
            open_files: HashMap::new(),
            lookups: HashMap::new(),
            unlinked: HashSet::new(),
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
                0o755
            } else {
                0o644
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
            perm: 0o755,
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
        nodes.nodes.get(&inode).map(|node| {
            let mut attr = self.entry_attr(node);
            if nodes.unlinked.contains(&inode) {
                attr.nlink = 0;
            }
            attr
        })
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

    fn readdir_entries(&self, inode: u64) -> Result<Vec<(u64, FileType, String)>> {
        let entries = self.directory_entries(inode)?;
        let parent = if inode == ROOT_INODE {
            ROOT_INODE
        } else {
            self.nodes
                .lock()
                .map_err(|_| "inode table lock poisoned")?
                .nodes
                .get(&inode)
                .ok_or("directory inode not found")?
                .parent
        };
        let mut all_entries = vec![
            (inode, FileType::Directory, ".".to_owned()),
            (parent, FileType::Directory, "..".to_owned()),
        ];
        all_entries.extend(entries);
        Ok(all_entries)
    }

    fn read_file(&self, inode: u64, offset: u64, size: u32) -> Result<Vec<u8>> {
        let (file_id, file_size) = {
            let nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
            let node = nodes.nodes.get(&inode).ok_or("file inode not found")?;
            if node.entry.is_directory() {
                return Err("cannot read a directory".into());
            }
            if let Some(data) = nodes.dirty.get(&inode) {
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(data.len());
                let end = start.saturating_add(size as usize).min(data.len());
                return Ok(data[start..end].to_vec());
            }
            (node.entry.resource_id.clone(), node.entry.size)
        };
        let size = (file_size.saturating_sub(offset)).min(size as u64) as u32;
        self.drive.download_file(&file_id, offset, size)
    }

    fn stage_file(&self, inode: u64) -> Result<()> {
        let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        if nodes.dirty.contains_key(&inode) {
            return Ok(());
        }
        let node = nodes.nodes.get(&inode).ok_or("file inode not found")?;
        if node.entry.is_directory() {
            return Err("cannot write a directory".into());
        }
        let remote = self.drive.file_metadata(&node.entry.resource_id)?;
        if !same_file_version(&node.entry, &remote) {
            return Err("MYBOX file changed remotely before editing".into());
        }
        let size = u32::try_from(node.entry.size).map_err(|_| "file too large to edit")?;
        let data = self.drive.download_file(&node.entry.resource_id, 0, size)?;
        if data.len() != size as usize {
            return Err("incomplete download; refusing to overwrite file".into());
        }
        nodes.dirty.insert(inode, data);
        nodes.baselines.insert(inode, remote);
        Ok(())
    }

    fn write_file(&self, inode: u64, offset: u64, data: &[u8]) -> Result<u32> {
        self.stage_file(inode)?;
        let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        let start = usize::try_from(offset).map_err(|_| "write offset too large")?;
        let end = start
            .checked_add(data.len())
            .ok_or("write offset too large")?;
        let buffer = nodes.dirty.get_mut(&inode).ok_or("file not staged")?;
        buffer.try_reserve(end.saturating_sub(buffer.len()))?;
        buffer.resize(end.max(buffer.len()), 0);
        buffer[start..end].copy_from_slice(data);
        let len = buffer.len() as u64;
        nodes
            .nodes
            .get_mut(&inode)
            .ok_or("file inode not found")?
            .entry
            .size = len;
        Ok(data.len() as u32)
    }

    fn truncate_file(&self, inode: u64, size: u64) -> Result<FileAttr> {
        if size == 0 {
            let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
            let node = nodes.nodes.get(&inode).ok_or("file inode not found")?;
            if node.entry.is_directory() {
                return Err("cannot truncate a directory".into());
            }
            if !nodes.dirty.contains_key(&inode) {
                let remote = self.drive.file_metadata(&node.entry.resource_id)?;
                if !same_file_version(&node.entry, &remote) {
                    return Err("MYBOX file changed remotely before truncation".into());
                }
                nodes.baselines.insert(inode, remote);
            }
            nodes.dirty.insert(inode, Vec::new());
            let node = nodes.nodes.get_mut(&inode).ok_or("file inode not found")?;
            node.entry.size = 0;
            return Ok(self.entry_attr(node));
        }
        self.stage_file(inode)?;
        let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        let size = usize::try_from(size).map_err(|_| "file too large")?;
        let buffer = nodes.dirty.get_mut(&inode).ok_or("file not staged")?;
        buffer.try_reserve(size.saturating_sub(buffer.len()))?;
        buffer.resize(size, 0);
        let node = nodes.nodes.get_mut(&inode).ok_or("file inode not found")?;
        node.entry.size = size as u64;
        Ok(self.entry_attr(node))
    }

    fn set_size(&self, inode: u64, size: u64, has_handle: bool) -> Result<FileAttr> {
        let attr = self.truncate_file(inode, size)?;
        if has_handle {
            return Ok(attr);
        }
        self.flush_file(inode)?;
        self.attr(inode)
            .ok_or_else(|| "file inode not found".into())
    }

    fn flush_file(&self, inode: u64) -> Result<()> {
        let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        if nodes.unlinked.contains(&inode) {
            return Ok(());
        }
        let Some(data) = nodes.dirty.get(&inode) else {
            return Ok(());
        };
        let node = nodes.nodes.get(&inode).ok_or("file inode not found")?;
        let baseline = nodes
            .baselines
            .get(&inode)
            .ok_or("missing staged file metadata")?;
        let remote = self.drive.file_metadata(&node.entry.resource_id)?;
        if !same_file_version(baseline, &remote) {
            return Err("MYBOX file changed remotely while editing".into());
        }
        let parent_id = nodes
            .nodes
            .get(&node.parent)
            .map(|parent| parent.entry.resource_id.as_str());
        let entry = self
            .drive
            .upload_file(parent_id, &node.entry.name, data.clone(), true)?;
        nodes
            .nodes
            .get_mut(&inode)
            .ok_or("file inode not found")?
            .entry = entry;
        nodes.dirty.remove(&inode);
        nodes.baselines.remove(&inode);
        Ok(())
    }

    fn create_file(&self, parent: u64, name: &OsStr) -> Result<FileAttr> {
        let name = name.to_str().ok_or("file name is not UTF-8")?;
        self.ensure_children(parent)?;
        let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        if nodes
            .children
            .get(&parent)
            .into_iter()
            .flatten()
            .any(|inode| nodes.nodes[inode].entry.name == name)
        {
            return Err("file already exists".into());
        }
        let parent_id = nodes
            .nodes
            .get(&parent)
            .map(|node| node.entry.resource_id.as_str());
        let entry = self.drive.upload_file(parent_id, name, Vec::new(), false)?;
        let inode = nodes.next_inode;
        nodes.next_inode += 1;
        let node = Node {
            inode,
            parent,
            entry,
        };
        let attr = self.entry_attr(&node);
        nodes.nodes.insert(inode, node);
        nodes.children.entry(parent).or_default().push(inode);
        Ok(attr)
    }

    fn unlink_file(&self, parent: u64, name: &OsStr) -> Result<()> {
        self.ensure_children(parent)?;
        let mut nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        let inode = *nodes
            .children
            .get(&parent)
            .ok_or("parent not found")?
            .iter()
            .find(|inode| nodes.nodes[inode].entry.name == name.to_string_lossy())
            .ok_or("file not found")?;
        let node = nodes.nodes.get(&inode).ok_or("file not found")?;
        if node.entry.is_directory() {
            return Err("cannot unlink a directory".into());
        }
        if nodes.open_files.get(&inode).copied().unwrap_or(0) > 0
            && !nodes.dirty.contains_key(&inode)
        {
            drop(nodes);
            self.stage_file(inode)?;
            nodes = self.nodes.lock().map_err(|_| "inode table lock poisoned")?;
        }
        let node = nodes.nodes.get(&inode).ok_or("file not found")?;
        self.drive.delete_file(&node.entry.resource_id)?;
        nodes
            .children
            .get_mut(&parent)
            .ok_or("parent not found")?
            .retain(|child| *child != inode);
        if nodes.open_files.get(&inode).copied().unwrap_or(0) > 0
            || nodes.lookups.get(&inode).copied().unwrap_or(0) > 0
        {
            nodes.unlinked.insert(inode);
        } else {
            nodes.nodes.remove(&inode);
            nodes.dirty.remove(&inode);
            nodes.baselines.remove(&inode);
        }
        Ok(())
    }

    fn open_file(&self, inode: u64) -> std::result::Result<(), Errno> {
        let mut nodes = self.nodes.lock().map_err(|_| Errno::EIO)?;
        let node = nodes.nodes.get(&inode).ok_or(Errno::ENOENT)?;
        if node.entry.is_directory() {
            return Err(Errno::EISDIR);
        }
        *nodes.open_files.entry(inode).or_default() += 1;
        Ok(())
    }

    fn close_file(&self, inode: u64) {
        let Ok(mut nodes) = self.nodes.lock() else {
            return;
        };
        if let Some(count) = nodes.open_files.get_mut(&inode) {
            *count -= 1;
            if *count == 0 {
                nodes.open_files.remove(&inode);
                Self::reap_unlinked(&mut nodes, inode);
            }
        }
    }

    fn remember_lookup(&self, inode: u64) {
        if let Ok(mut nodes) = self.nodes.lock() {
            *nodes.lookups.entry(inode).or_default() += 1;
        }
    }

    fn forget_file(&self, inode: u64, nlookup: u64) {
        let Ok(mut nodes) = self.nodes.lock() else {
            return;
        };
        if let Some(count) = nodes.lookups.get_mut(&inode) {
            *count = count.saturating_sub(nlookup);
            if *count == 0 {
                nodes.lookups.remove(&inode);
            }
        }
        Self::reap_unlinked(&mut nodes, inode);
    }

    fn reap_unlinked(nodes: &mut NodeTable, inode: u64) {
        if nodes.unlinked.contains(&inode)
            && !nodes.open_files.contains_key(&inode)
            && !nodes.lookups.contains_key(&inode)
        {
            nodes.unlinked.remove(&inode);
            nodes.nodes.remove(&inode);
            nodes.dirty.remove(&inode);
            nodes.baselines.remove(&inode);
        }
    }

    fn mkdir_folder(&self, parent: u64, name: &OsStr) -> std::result::Result<FileAttr, Errno> {
        let name = valid_name(name)?;
        self.ensure_directory(parent)?;
        self.ensure_children(parent).map_err(|error| {
            tracing::warn!(%error, "MYBOX parent listing failed");
            Errno::EIO
        })?;
        let mut nodes = self.nodes.lock().map_err(|_| Errno::EIO)?;
        let parent_id = if parent == ROOT_INODE {
            None
        } else {
            let parent_node = nodes.nodes.get(&parent).ok_or(Errno::ENOENT)?;
            if !parent_node.entry.is_directory() {
                return Err(Errno::ENOTDIR);
            }
            Some(parent_node.entry.resource_id.as_str())
        };
        if child_inode(&nodes, parent, name).is_some() {
            return Err(Errno::EEXIST);
        }
        let entry = self.drive.create_folder(parent_id, name).map_err(|error| {
            tracing::warn!(%error, "MYBOX folder creation failed");
            Errno::EIO
        })?;
        let inode = nodes.next_inode;
        nodes.next_inode += 1;
        let node = Node {
            inode,
            parent,
            entry,
        };
        let attr = self.entry_attr(&node);
        nodes.nodes.insert(inode, node);
        nodes.children.entry(parent).or_default().push(inode);
        nodes.children.insert(inode, Vec::new());
        nodes.loaded_directories.insert(inode);
        Ok(attr)
    }

    fn rmdir_folder(&self, parent: u64, name: &OsStr) -> std::result::Result<(), Errno> {
        let name = valid_name(name)?;
        self.ensure_directory(parent)?;
        self.ensure_children(parent).map_err(|_| Errno::EIO)?;
        let inode = {
            let nodes = self.nodes.lock().map_err(|_| Errno::EIO)?;
            let inode = child_inode(&nodes, parent, name).ok_or(Errno::ENOENT)?;
            if !nodes.nodes[&inode].entry.is_directory() {
                return Err(Errno::ENOTDIR);
            }
            inode
        };
        self.ensure_children(inode).map_err(|error| {
            tracing::warn!(%error, "MYBOX folder listing failed");
            Errno::EIO
        })?;
        let mut nodes = self.nodes.lock().map_err(|_| Errno::EIO)?;
        if !nodes.children.get(&inode).ok_or(Errno::ENOENT)?.is_empty() {
            return Err(Errno::ENOTEMPTY);
        }
        let resource_id = &nodes
            .nodes
            .get(&inode)
            .ok_or(Errno::ENOENT)?
            .entry
            .resource_id;
        let remote_children = self.drive.list_children(resource_id).map_err(|error| {
            tracing::warn!(%error, "MYBOX folder listing failed before deletion");
            Errno::EIO
        })?;
        if !remote_children.is_empty() {
            return Err(Errno::ENOTEMPTY);
        }
        self.drive.delete_file(resource_id).map_err(|error| {
            tracing::warn!(%error, "MYBOX folder deletion failed");
            Errno::EIO
        })?;
        nodes
            .children
            .get_mut(&parent)
            .ok_or(Errno::ENOENT)?
            .retain(|child| *child != inode);
        nodes.children.remove(&inode);
        nodes.loaded_directories.remove(&inode);
        if nodes.lookups.get(&inode).copied().unwrap_or(0) > 0 {
            nodes.unlinked.insert(inode);
        } else {
            nodes.nodes.remove(&inode);
        }
        Ok(())
    }

    fn rename_path(
        &self,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
    ) -> std::result::Result<(), Errno> {
        let name = valid_name(name)?;
        let newname = valid_name(newname)?;
        self.ensure_directory(parent)?;
        self.ensure_directory(newparent)?;
        self.ensure_children(parent).map_err(|_| Errno::EIO)?;
        self.ensure_children(newparent).map_err(|_| Errno::EIO)?;
        let mut nodes = self.nodes.lock().map_err(|_| Errno::EIO)?;
        let inode = child_inode(&nodes, parent, name).ok_or(Errno::ENOENT)?;
        if parent == newparent && name == newname {
            return Ok(());
        }
        if child_inode(&nodes, newparent, newname).is_some() {
            return Err(Errno::EEXIST);
        }
        if parent != newparent && child_inode(&nodes, newparent, name).is_some() {
            return Err(Errno::EEXIST);
        }
        let mut ancestor = newparent;
        while ancestor != ROOT_INODE {
            if ancestor == inode {
                return Err(Errno::EINVAL);
            }
            ancestor = nodes.nodes.get(&ancestor).ok_or(Errno::ENOENT)?.parent;
        }
        let resource_id = nodes
            .nodes
            .get(&inode)
            .ok_or(Errno::ENOENT)?
            .entry
            .resource_id
            .clone();
        if parent != newparent {
            let parent_id = nodes
                .nodes
                .get(&newparent)
                .map(|node| node.entry.resource_id.as_str());
            self.drive
                .move_resource(&resource_id, parent_id)
                .map_err(|error| {
                    tracing::warn!(%error, "MYBOX resource move failed");
                    Errno::EIO
                })?;
            nodes
                .children
                .get_mut(&parent)
                .ok_or(Errno::EIO)?
                .retain(|child| *child != inode);
            nodes
                .children
                .get_mut(&newparent)
                .ok_or(Errno::EIO)?
                .push(inode);
            nodes.nodes.get_mut(&inode).ok_or(Errno::EIO)?.parent = newparent;
        }
        if name != newname {
            self.drive
                .rename_resource(&resource_id, newname)
                .map_err(|error| {
                    tracing::warn!(%error, "MYBOX resource rename failed after move");
                    Errno::EIO
                })?;
            nodes.nodes.get_mut(&inode).ok_or(Errno::EIO)?.entry.name = newname.to_owned();
        }
        Ok(())
    }

    fn ensure_directory(&self, inode: u64) -> std::result::Result<(), Errno> {
        let attr = self.attr(inode).ok_or(Errno::ENOENT)?;
        if attr.kind != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        Ok(())
    }
}

fn valid_name(name: &OsStr) -> std::result::Result<&str, Errno> {
    let name = name.to_str().ok_or(Errno::EINVAL)?;
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(Errno::EINVAL);
    }
    Ok(name)
}

fn same_file_version(cached: &RemoteEntry, current: &RemoteEntry) -> bool {
    cached.size == current.size
        && cached
            .modified_at
            .as_ref()
            .is_none_or(|modified| current.modified_at.as_ref() == Some(modified))
}

fn child_inode(nodes: &NodeTable, parent: u64, name: &str) -> Option<u64> {
    nodes.children.get(&parent)?.iter().copied().find(|inode| {
        nodes
            .nodes
            .get(inode)
            .is_some_and(|node| node.entry.name == name)
    })
}

impl<D: RemoteDrive> Filesystem for MyboxFs<D> {
    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        self.forget_file(ino.0, nlookup);
    }
    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        if mode & !umask & 0o777 != 0o755 {
            reply.error(Errno::EOPNOTSUPP);
            return;
        }
        match self.mkdir_folder(parent.0, name) {
            Ok(attr) => {
                self.remember_lookup(attr.ino.0);
                reply.entry(&ATTR_TTL, &attr, Generation(0))
            }
            Err(error) => reply.error(error),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.rmdir_folder(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: fuser::RenameFlags,
        reply: ReplyEmpty,
    ) {
        if !flags.is_empty() {
            reply.error(Errno::EINVAL);
            return;
        }
        match self.rename_path(parent.0, name, newparent.0, newname) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        if mode & !umask & 0o777 != 0o644 {
            reply.error(Errno::EOPNOTSUPP);
            return;
        }
        match self.lookup_entry(parent.0, name) {
            Ok(Some(_)) => {
                reply.error(Errno::EEXIST);
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "MYBOX file creation lookup failed");
                reply.error(Errno::EIO);
                return;
            }
            Ok(None) => {}
        }
        match self.create_file(parent.0, name) {
            Ok(attr) => match self.open_file(attr.ino.0) {
                Ok(()) => {
                    self.remember_lookup(attr.ino.0);
                    reply.created(
                        &ATTR_TTL,
                        &attr,
                        Generation(0),
                        FileHandle(0),
                        FopenFlags::empty(),
                    )
                }
                Err(error) => reply.error(error),
            },
            Err(error) => {
                tracing::warn!(%error, "MYBOX file creation failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.lookup_entry(parent.0, name) {
            Ok(None) => {
                reply.error(Errno::ENOENT);
                return;
            }
            Ok(Some(attr)) if attr.kind == FileType::Directory => {
                reply.error(Errno::EISDIR);
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "MYBOX file deletion lookup failed");
                reply.error(Errno::EIO);
                return;
            }
            Ok(Some(_)) => {}
        }
        match self.unlink_file(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(error) => {
                tracing::warn!(%error, "MYBOX file deletion failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<fuser::TimeOrNow>,
        mtime: Option<fuser::TimeOrNow>,
        ctime: Option<SystemTime>,
        fh: Option<FileHandle>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        if mode.is_some()
            || uid.is_some()
            || gid.is_some()
            || atime.is_some()
            || mtime.is_some()
            || ctime.is_some()
            || crtime.is_some()
            || chgtime.is_some()
            || bkuptime.is_some()
            || flags.is_some()
        {
            reply.error(Errno::EOPNOTSUPP);
            return;
        }
        let attr = match size {
            Some(size) => self.set_size(ino.0, size, fh.is_some()),
            None => self
                .attr(ino.0)
                .ok_or_else(|| "file inode not found".into()),
        };
        match attr {
            Ok(attr) => reply.attr(&ATTR_TTL, &attr),
            Err(error) => {
                tracing::warn!(%error, "MYBOX attribute update failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyWrite,
    ) {
        match self.write_file(ino.0, offset, data) {
            Ok(size) => reply.written(size),
            Err(error) => {
                tracing::warn!(%error, "MYBOX file write failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn flush(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _lock_owner: fuser::LockOwner,
        reply: ReplyEmpty,
    ) {
        match self.flush_file(ino.0) {
            Ok(()) => reply.ok(),
            Err(error) => {
                tracing::warn!(%error, "MYBOX file flush failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn fsync(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        match self.flush_file(ino.0) {
            Ok(()) => reply.ok(),
            Err(error) => {
                tracing::warn!(%error, "MYBOX file sync failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let result = self.flush_file(ino.0);
        self.close_file(ino.0);
        match result {
            Ok(()) => reply.ok(),
            Err(error) => {
                tracing::warn!(%error, "MYBOX file release failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.lookup_entry(parent.0, name) {
            Ok(Some(attr)) => {
                self.remember_lookup(attr.ino.0);
                reply.entry(&ATTR_TTL, &attr, Generation(0))
            }
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
        match self.open_file(ino.0) {
            Ok(()) => reply.opened(FileHandle(0), FopenFlags::empty()),
            Err(error) => reply.error(error),
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
        match self.readdir_entries(ino.0) {
            Ok(all_entries) => {
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
    config.mount_options = vec![
        MountOption::DefaultPermissions,
        MountOption::FSName("myboxfs".into()),
    ];
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
        root: Mutex<Vec<RemoteEntry>>,
        children: Mutex<HashMap<String, Vec<RemoteEntry>>>,
        files: Mutex<HashMap<String, Vec<u8>>>,
        downloads: Mutex<Vec<(String, u64, u32)>>,
        fail_upload: Mutex<bool>,
        fail_delete: Mutex<bool>,
        fail_move: Mutex<bool>,
        fail_rename: Mutex<bool>,
    }

    impl RemoteDrive for FakeDrive {
        fn list_root(&self) -> Result<Vec<RemoteEntry>> {
            Ok(self.root.lock().unwrap().clone())
        }
        fn list_children(&self, folder_id: &str) -> Result<Vec<RemoteEntry>> {
            Ok(self
                .children
                .lock()
                .unwrap()
                .get(folder_id)
                .cloned()
                .unwrap_or_default())
        }
        fn file_metadata(&self, file_id: &str) -> Result<RemoteEntry> {
            let root = self.root.lock().unwrap();
            let children = self.children.lock().unwrap();
            let mut entry = root
                .iter()
                .chain(children.values().flatten())
                .find(|entry| entry.resource_id == file_id)
                .ok_or("file not found")?
                .clone();
            entry.size = self
                .files
                .lock()
                .unwrap()
                .get(file_id)
                .ok_or("file data missing")?
                .len() as u64;
            Ok(entry)
        }
        fn create_folder(&self, parent_id: Option<&str>, name: &str) -> Result<RemoteEntry> {
            let entry = entry(&format!("folder-{name}"), name, 0, "folder");
            let mut root = self.root.lock().unwrap();
            let mut children = self.children.lock().unwrap();
            let siblings = match parent_id {
                Some(parent_id) => children.entry(parent_id.to_owned()).or_default(),
                None => &mut root,
            };
            if siblings.iter().any(|existing| existing.name == name) {
                return Err("folder exists".into());
            }
            siblings.push(entry.clone());
            Ok(entry)
        }
        fn download_file(&self, file_id: &str, offset: u64, size: u32) -> Result<Vec<u8>> {
            self.downloads
                .lock()
                .unwrap()
                .push((file_id.to_owned(), offset, size));
            let files = self.files.lock().unwrap();
            let file = files.get(file_id).ok_or("missing test file")?;
            let start = (offset as usize).min(file.len());
            let end = start.saturating_add(size as usize).min(file.len());
            Ok(file[start..end].to_vec())
        }
        fn upload_file(
            &self,
            parent_id: Option<&str>,
            name: &str,
            data: Vec<u8>,
            overwrite: bool,
        ) -> Result<RemoteEntry> {
            if *self.fail_upload.lock().unwrap() {
                return Err("upload failed".into());
            }
            let id = format!("file-{name}");
            let entry = entry(&id, name, data.len() as u64, "file");
            let mut root = self.root.lock().unwrap();
            let mut children = self.children.lock().unwrap();
            let siblings = match parent_id {
                Some(parent_id) => children.entry(parent_id.to_owned()).or_default(),
                None => &mut root,
            };
            if let Some(existing) = siblings.iter_mut().find(|existing| existing.name == name) {
                if !overwrite {
                    return Err("file exists".into());
                }
                *existing = entry.clone();
            } else {
                siblings.push(entry.clone());
            }
            self.files.lock().unwrap().insert(id, data);
            Ok(entry)
        }
        fn delete_file(&self, file_id: &str) -> Result<()> {
            if *self.fail_delete.lock().unwrap() {
                return Err("delete failed".into());
            }
            self.root
                .lock()
                .unwrap()
                .retain(|entry| entry.resource_id != file_id);
            for entries in self.children.lock().unwrap().values_mut() {
                entries.retain(|entry| entry.resource_id != file_id);
            }
            self.files.lock().unwrap().remove(file_id);
            Ok(())
        }
        fn move_resource(&self, resource_id: &str, parent_id: Option<&str>) -> Result<()> {
            if *self.fail_move.lock().unwrap() {
                return Err("move failed".into());
            }
            let mut root = self.root.lock().unwrap();
            let mut children = self.children.lock().unwrap();
            let entry = if let Some(index) = root
                .iter()
                .position(|entry| entry.resource_id == resource_id)
            {
                root.remove(index)
            } else {
                let siblings = children
                    .values_mut()
                    .find(|siblings| {
                        siblings
                            .iter()
                            .any(|entry| entry.resource_id == resource_id)
                    })
                    .ok_or("resource not found")?;
                let index = siblings
                    .iter()
                    .position(|entry| entry.resource_id == resource_id)
                    .unwrap();
                siblings.remove(index)
            };
            match parent_id {
                Some(parent_id) => children
                    .entry(parent_id.to_owned())
                    .or_default()
                    .push(entry),
                None => root.push(entry),
            }
            Ok(())
        }
        fn rename_resource(&self, resource_id: &str, name: &str) -> Result<()> {
            if *self.fail_rename.lock().unwrap() {
                return Err("rename failed".into());
            }
            let mut root = self.root.lock().unwrap();
            let mut children = self.children.lock().unwrap();
            let entry = root
                .iter_mut()
                .chain(children.values_mut().flatten())
                .find(|entry| entry.resource_id == resource_id)
                .ok_or("resource not found")?;
            entry.name = name.to_owned();
            Ok(())
        }
    }

    fn entry(id: &str, name: &str, size: u64, kind: &str) -> RemoteEntry {
        RemoteEntry {
            resource_id: id.into(),
            name: name.into(),
            size,
            kind: kind.into(),
            modified_at: None,
        }
    }

    fn filesystem() -> MyboxFs<FakeDrive> {
        MyboxFs::new(FakeDrive {
            root: Mutex::new(vec![
                entry("folder-1", "Documents", 0, "folder"),
                entry("file-1", "hello.txt", 11, "file"),
            ]),
            children: Mutex::new(HashMap::from([(
                "folder-1".into(),
                vec![entry("file-2", "notes.txt", 5, "file")],
            )])),
            files: Mutex::new(HashMap::from([
                ("file-1".into(), b"hello world".to_vec()),
                ("file-2".into(), b"notes".to_vec()),
            ])),
            downloads: Mutex::new(Vec::new()),
            fail_upload: Mutex::new(false),
            fail_delete: Mutex::new(false),
            fail_move: Mutex::new(false),
            fail_rename: Mutex::new(false),
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
        assert_eq!(documents.perm, 0o755);
        assert_eq!(hello.kind, FileType::RegularFile);
        assert_eq!(hello.size, 11);
        assert_eq!(hello.perm, 0o644);

        let children = filesystem.directory_entries(documents.ino.0).unwrap();
        assert_eq!(
            children,
            vec![(4, FileType::RegularFile, "notes.txt".into())]
        );
        let entries = filesystem.readdir_entries(documents.ino.0).unwrap();
        assert_eq!(
            entries[0],
            (documents.ino.0, FileType::Directory, ".".into())
        );
        assert_eq!(entries[1], (ROOT_INODE, FileType::Directory, "..".into()));
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

    #[test]
    fn creates_writes_flushes_and_deletes_files() {
        let filesystem = filesystem();
        let documents = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("Documents"))
            .unwrap()
            .unwrap();
        let created = filesystem
            .create_file(documents.ino.0, OsStr::new("draft.txt"))
            .unwrap();
        assert_eq!(created.size, 0);
        assert_eq!(
            filesystem.write_file(created.ino.0, 0, b"hello").unwrap(),
            5
        );
        assert_eq!(
            filesystem.write_file(created.ino.0, 6, b"world").unwrap(),
            5
        );
        assert_eq!(
            filesystem.read_file(created.ino.0, 0, 20).unwrap(),
            b"hello\0world"
        );
        assert_eq!(filesystem.attr(created.ino.0).unwrap().size, 11);
        filesystem.flush_file(created.ino.0).unwrap();
        assert_eq!(
            filesystem.read_file(created.ino.0, 0, 20).unwrap(),
            b"hello\0world"
        );
        assert_eq!(
            filesystem
                .lookup_entry(documents.ino.0, OsStr::new("draft.txt"))
                .unwrap()
                .unwrap()
                .ino,
            created.ino
        );

        filesystem.truncate_file(created.ino.0, 3).unwrap();
        filesystem.flush_file(created.ino.0).unwrap();
        assert_eq!(filesystem.read_file(created.ino.0, 0, 20).unwrap(), b"hel");
        filesystem
            .unlink_file(documents.ino.0, OsStr::new("draft.txt"))
            .unwrap();
        assert!(
            filesystem
                .lookup_entry(documents.ino.0, OsStr::new("draft.txt"))
                .unwrap()
                .is_none()
        );
        assert!(filesystem.attr(created.ino.0).is_none());
    }

    #[test]
    fn modifies_existing_file_without_discarding_other_bytes() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem.write_file(file.ino.0, 6, b"rust!").unwrap();
        assert_eq!(
            filesystem.read_file(file.ino.0, 0, 20).unwrap(),
            b"hello rust!"
        );
        filesystem.flush_file(file.ino.0).unwrap();
        assert_eq!(
            filesystem.read_file(file.ino.0, 0, 20).unwrap(),
            b"hello rust!"
        );
        filesystem
            .unlink_file(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap();
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn failed_upload_and_delete_keep_local_state() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem.write_file(file.ino.0, 0, b"H").unwrap();
        *filesystem.drive.fail_upload.lock().unwrap() = true;
        assert!(filesystem.flush_file(file.ino.0).is_err());
        assert_eq!(
            filesystem.read_file(file.ino.0, 0, 20).unwrap(),
            b"Hello world"
        );
        *filesystem.drive.fail_upload.lock().unwrap() = false;
        filesystem.flush_file(file.ino.0).unwrap();
        *filesystem.drive.fail_delete.lock().unwrap() = true;
        assert!(
            filesystem
                .unlink_file(ROOT_INODE, OsStr::new("hello.txt"))
                .is_err()
        );
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn refuses_to_overwrite_an_incompletely_downloaded_file() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem
            .drive
            .files
            .lock()
            .unwrap()
            .insert("file-1".into(), b"short".to_vec());
        assert!(filesystem.write_file(file.ino.0, 0, b"changed").is_err());
        assert!(
            !filesystem
                .nodes
                .lock()
                .unwrap()
                .dirty
                .contains_key(&file.ino.0)
        );
    }

    #[test]
    fn truncating_to_zero_does_not_download_old_contents() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(filesystem.truncate_file(file.ino.0, 0).unwrap().size, 0);
        assert!(filesystem.drive.downloads.lock().unwrap().is_empty());
        filesystem
            .write_file(file.ino.0, 0, b"replacement")
            .unwrap();
        filesystem.flush_file(file.ino.0).unwrap();
        assert_eq!(
            filesystem.read_file(file.ino.0, 0, 20).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn path_based_truncate_persists_and_reports_upload_failure() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(filesystem.set_size(file.ino.0, 5, false).unwrap().size, 5);
        assert_eq!(
            filesystem.drive.files.lock().unwrap()["file-hello.txt"],
            b"hello"
        );
        *filesystem.drive.fail_upload.lock().unwrap() = true;
        assert!(filesystem.set_size(file.ino.0, 0, false).is_err());
        assert_eq!(
            filesystem.drive.files.lock().unwrap()["file-hello.txt"],
            b"hello"
        );
    }

    #[test]
    fn open_unlinked_file_survives_until_last_release() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem.open_file(file.ino.0).unwrap();
        filesystem.open_file(file.ino.0).unwrap();
        filesystem.write_file(file.ino.0, 0, b"H").unwrap();
        filesystem
            .unlink_file(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap();
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            filesystem.read_file(file.ino.0, 0, 11).unwrap(),
            b"Hello world"
        );
        filesystem.write_file(file.ino.0, 6, b"there").unwrap();
        filesystem.flush_file(file.ino.0).unwrap();
        assert!(
            !filesystem
                .drive
                .files
                .lock()
                .unwrap()
                .contains_key("file-1")
        );
        filesystem.close_file(file.ino.0);
        assert!(filesystem.attr(file.ino.0).is_some());
        filesystem.close_file(file.ino.0);
        assert!(filesystem.attr(file.ino.0).is_none());
    }

    #[test]
    fn unlinked_inodes_wait_for_lookup_forget() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem.remember_lookup(file.ino.0);
        filesystem.open_file(file.ino.0).unwrap();
        filesystem
            .unlink_file(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap();
        filesystem.close_file(file.ino.0);
        assert_eq!(filesystem.attr(file.ino.0).unwrap().nlink, 0);
        filesystem.forget_file(file.ino.0, 1);
        assert!(filesystem.attr(file.ino.0).is_none());
        let folder = filesystem
            .mkdir_folder(ROOT_INODE, OsStr::new("Temporary"))
            .unwrap();
        filesystem.remember_lookup(folder.ino.0);
        filesystem
            .rmdir_folder(ROOT_INODE, OsStr::new("Temporary"))
            .unwrap();
        assert_eq!(filesystem.attr(folder.ino.0).unwrap().nlink, 0);
        filesystem.forget_file(folder.ino.0, 1);
        assert!(filesystem.attr(folder.ino.0).is_none());
    }

    #[test]
    fn refuses_to_overwrite_a_remotely_grown_file() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem
            .drive
            .files
            .lock()
            .unwrap()
            .insert("file-1".into(), b"hello world extra".to_vec());
        assert!(filesystem.write_file(file.ino.0, 0, b"H").is_err());
        assert!(
            !filesystem
                .nodes
                .lock()
                .unwrap()
                .dirty
                .contains_key(&file.ino.0)
        );
        assert_eq!(
            filesystem.drive.files.lock().unwrap()["file-1"],
            b"hello world extra"
        );
    }

    #[test]
    fn refuses_to_flush_changes_when_remote_version_changes() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem.write_file(file.ino.0, 0, b"H").unwrap();
        filesystem
            .drive
            .files
            .lock()
            .unwrap()
            .insert("file-1".into(), b"hello world extra".to_vec());
        assert!(filesystem.flush_file(file.ino.0).is_err());
        assert_eq!(
            filesystem.read_file(file.ino.0, 0, 11).unwrap(),
            b"Hello world"
        );
        assert_eq!(
            filesystem.drive.files.lock().unwrap()["file-1"],
            b"hello world extra"
        );
    }

    #[test]
    fn refuses_equal_size_edits_when_remote_modification_time_changes() {
        let filesystem = filesystem();
        filesystem.drive.root.lock().unwrap()[1].modified_at = Some("before".into());
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        filesystem.write_file(file.ino.0, 0, b"H").unwrap();
        filesystem.drive.root.lock().unwrap()[1].modified_at = Some("after".into());
        assert!(filesystem.flush_file(file.ino.0).is_err());
        assert_eq!(
            filesystem.drive.files.lock().unwrap()["file-1"],
            b"hello world"
        );
    }

    #[test]
    fn creates_and_removes_only_empty_folders() {
        let filesystem = filesystem();
        let folder = filesystem
            .mkdir_folder(ROOT_INODE, OsStr::new("New"))
            .unwrap();
        assert_eq!(folder.kind, FileType::Directory);
        assert!(
            filesystem
                .directory_entries(folder.ino.0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            filesystem
                .mkdir_folder(ROOT_INODE, OsStr::new("New"))
                .unwrap_err(),
            Errno::EEXIST
        );
        assert_eq!(
            filesystem.rmdir_folder(ROOT_INODE, OsStr::new("Documents")),
            Err(Errno::ENOTEMPTY)
        );
        filesystem
            .rmdir_folder(ROOT_INODE, OsStr::new("New"))
            .unwrap();
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("New"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            filesystem.rmdir_folder(ROOT_INODE, OsStr::new("hello.txt")),
            Err(Errno::ENOTDIR)
        );
    }

    #[test]
    fn moves_and_renames_files_and_folders_without_changing_inodes() {
        let filesystem = filesystem();
        let documents = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("Documents"))
            .unwrap()
            .unwrap();
        let notes = filesystem
            .lookup_entry(documents.ino.0, OsStr::new("notes.txt"))
            .unwrap()
            .unwrap();
        filesystem
            .rename_path(
                documents.ino.0,
                OsStr::new("notes.txt"),
                documents.ino.0,
                OsStr::new("renamed.txt"),
            )
            .unwrap();
        filesystem
            .rename_path(
                documents.ino.0,
                OsStr::new("renamed.txt"),
                ROOT_INODE,
                OsStr::new("moved.txt"),
            )
            .unwrap();
        assert_eq!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("moved.txt"))
                .unwrap()
                .unwrap()
                .ino,
            notes.ino
        );
        assert!(
            filesystem
                .lookup_entry(documents.ino.0, OsStr::new("renamed.txt"))
                .unwrap()
                .is_none()
        );
        filesystem
            .rename_path(
                ROOT_INODE,
                OsStr::new("Documents"),
                ROOT_INODE,
                OsStr::new("Archive"),
            )
            .unwrap();
        assert_eq!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("Archive"))
                .unwrap()
                .unwrap()
                .ino,
            documents.ino
        );
    }

    #[test]
    fn rejects_rename_collisions_and_cycles() {
        let filesystem = filesystem();
        let documents = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("Documents"))
            .unwrap()
            .unwrap();
        let nested = filesystem
            .mkdir_folder(documents.ino.0, OsStr::new("Nested"))
            .unwrap();
        assert_eq!(
            filesystem.rename_path(
                ROOT_INODE,
                OsStr::new("Documents"),
                nested.ino.0,
                OsStr::new("Cycle")
            ),
            Err(Errno::EINVAL)
        );
        assert_eq!(
            filesystem.rename_path(
                ROOT_INODE,
                OsStr::new("hello.txt"),
                documents.ino.0,
                OsStr::new("notes.txt")
            ),
            Err(Errno::EEXIST)
        );
        assert_eq!(
            filesystem.rename_path(
                ROOT_INODE,
                OsStr::new("hello.txt"),
                ROOT_INODE,
                OsStr::new("Documents")
            ),
            Err(Errno::EEXIST)
        );
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn keeps_local_links_in_sync_with_partial_remote_failures() {
        let filesystem = filesystem();
        let documents = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("Documents"))
            .unwrap()
            .unwrap();
        *filesystem.drive.fail_move.lock().unwrap() = true;
        assert_eq!(
            filesystem.rename_path(
                ROOT_INODE,
                OsStr::new("hello.txt"),
                documents.ino.0,
                OsStr::new("new.txt")
            ),
            Err(Errno::EIO)
        );
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
                .unwrap()
                .is_some()
        );
        *filesystem.drive.fail_move.lock().unwrap() = false;
        *filesystem.drive.fail_rename.lock().unwrap() = true;
        assert_eq!(
            filesystem.rename_path(
                ROOT_INODE,
                OsStr::new("hello.txt"),
                documents.ino.0,
                OsStr::new("new.txt")
            ),
            Err(Errno::EIO)
        );
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
                .unwrap()
                .is_none()
        );
        assert!(
            filesystem
                .lookup_entry(documents.ino.0, OsStr::new("hello.txt"))
                .unwrap()
                .is_some()
        );
        assert!(
            filesystem
                .lookup_entry(documents.ino.0, OsStr::new("new.txt"))
                .unwrap()
                .is_none()
        );
        *filesystem.drive.fail_delete.lock().unwrap() = true;
        let empty = filesystem
            .mkdir_folder(ROOT_INODE, OsStr::new("Empty"))
            .unwrap();
        assert_eq!(
            filesystem.rmdir_folder(ROOT_INODE, OsStr::new("Empty")),
            Err(Errno::EIO)
        );
        assert!(filesystem.attr(empty.ino.0).is_some());
    }

    #[test]
    fn moving_a_loaded_folder_preserves_its_descendants() {
        let filesystem = filesystem();
        let documents = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("Documents"))
            .unwrap()
            .unwrap();
        let notes = filesystem
            .lookup_entry(documents.ino.0, OsStr::new("notes.txt"))
            .unwrap()
            .unwrap();
        let destination = filesystem
            .mkdir_folder(ROOT_INODE, OsStr::new("Destination"))
            .unwrap();
        filesystem
            .rename_path(
                ROOT_INODE,
                OsStr::new("Documents"),
                destination.ino.0,
                OsStr::new("Documents"),
            )
            .unwrap();
        assert!(
            filesystem
                .lookup_entry(ROOT_INODE, OsStr::new("Documents"))
                .unwrap()
                .is_none()
        );
        let moved = filesystem
            .lookup_entry(destination.ino.0, OsStr::new("Documents"))
            .unwrap()
            .unwrap();
        assert_eq!(moved.ino, documents.ino);
        assert_eq!(
            filesystem
                .lookup_entry(moved.ino.0, OsStr::new("notes.txt"))
                .unwrap()
                .unwrap()
                .ino,
            notes.ino
        );
        assert_eq!(
            filesystem.nodes.lock().unwrap().nodes[&documents.ino.0].parent,
            destination.ino.0
        );
        assert_eq!(
            filesystem.rmdir_folder(destination.ino.0, OsStr::new("Documents")),
            Err(Errno::ENOTEMPTY)
        );
    }

    #[test]
    fn checks_remote_emptiness_and_parent_types_before_mutating() {
        let filesystem = filesystem();
        let file = filesystem
            .lookup_entry(ROOT_INODE, OsStr::new("hello.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(
            filesystem
                .mkdir_folder(file.ino.0, OsStr::new("Invalid"))
                .unwrap_err(),
            Errno::ENOTDIR
        );
        assert_eq!(
            filesystem.rmdir_folder(file.ino.0, OsStr::new("Invalid")),
            Err(Errno::ENOTDIR)
        );
        assert_eq!(
            filesystem.rename_path(
                ROOT_INODE,
                OsStr::new("Documents"),
                file.ino.0,
                OsStr::new("Invalid")
            ),
            Err(Errno::ENOTDIR)
        );
        let folder = filesystem
            .mkdir_folder(ROOT_INODE, OsStr::new("New"))
            .unwrap();
        filesystem.drive.children.lock().unwrap().insert(
            "folder-New".into(),
            vec![entry("outside", "outside.txt", 1, "file")],
        );
        assert_eq!(
            filesystem.rmdir_folder(ROOT_INODE, OsStr::new("New")),
            Err(Errno::ENOTEMPTY)
        );
        assert!(filesystem.attr(folder.ino.0).is_some());
    }
}
