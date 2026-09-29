use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(2);

struct CachedRange {
    inode: u64,
    offset: u64,
    bytes: Vec<u8>,
    expires: Instant,
}

#[derive(Default)]
pub struct ReadCache {
    ranges: VecDeque<CachedRange>,
    bytes: usize,
}

pub type MetadataCache = ReadCache;

impl ReadCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&mut self, inode: u64, offset: u64, size: u32) -> Option<Vec<u8>> {
        self.expire();
        let end = offset.checked_add(size as u64)?;
        self.ranges.iter().find_map(|range| {
            let start = usize::try_from(offset.checked_sub(range.offset)?).ok()?;
            let finish = usize::try_from(end.checked_sub(range.offset)?).ok()?;
            (range.inode == inode && finish <= range.bytes.len())
                .then(|| range.bytes[start..finish].to_vec())
        })
    }

    pub fn insert(&mut self, inode: u64, offset: u64, bytes: Vec<u8>) {
        if bytes.len() > MAX_BYTES {
            return;
        }
        self.expire();
        while self.bytes + bytes.len() > MAX_BYTES {
            if let Some(oldest) = self.ranges.pop_front() {
                self.bytes -= oldest.bytes.len();
            }
        }
        self.bytes += bytes.len();
        self.ranges.push_back(CachedRange {
            inode,
            offset,
            bytes,
            expires: Instant::now() + TTL,
        });
    }

    pub fn invalidate(&mut self, inode: u64) {
        self.ranges.retain(|range| range.inode != inode);
        self.bytes = self.ranges.iter().map(|range| range.bytes.len()).sum();
    }

    fn expire(&mut self) {
        let now = Instant::now();
        self.ranges.retain(|range| range.expires > now);
        self.bytes = self.ranges.iter().map(|range| range.bytes.len()).sum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_and_invalidates_ranges() {
        let mut cache = ReadCache::default();
        cache.insert(1, 4, b"hello".to_vec());
        assert_eq!(cache.get(1, 5, 3), Some(b"ell".to_vec()));
        cache.insert(2, 0, vec![0; MAX_BYTES]);
        assert_eq!(cache.get(1, 5, 3), None);
        cache.invalidate(2);
        assert_eq!(cache.get(2, 0, 1), None);
    }
}
