//! Read byte ranges of a remote file with HTTP range requests.
//!
//! Update packages are large (up to several GiB), but the tool needs only small parts of them:
//! the directory of a WIM, an index, and a few payload ranges. `HttpFile` reads aligned blocks
//! and keeps a bounded number of them in memory.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::{Arc, Mutex};

/// The size of one cached block.
const BLOCK_SIZE: u64 = 1 << 20;
/// The number of blocks in the cache.
const CACHE_BLOCKS: usize = 64;

/// A remote file that supports HTTP range requests.
pub struct HttpFile {
    agent: ureq::Agent,
    url: String,
    len: u64,
    cache: Mutex<Cache>,
}

/// The block cache, with the least recently loaded block first.
#[derive(Default)]
struct Cache {
    blocks: BTreeMap<u64, Arc<[u8]>>,
    order: VecDeque<u64>,
}

impl HttpFile {
    /// Open a remote file and read its length.
    ///
    /// # Errors
    ///
    /// Returns an error if the server does not answer a range request with its total length.
    pub fn open(agent: &ureq::Agent, url: &str) -> io::Result<Self> {
        let response = agent
            .get(url)
            .header("Range", "bytes=0-0")
            .call()
            .map_err(io::Error::other)?;
        let len = response
            .headers()
            .get("Content-Range")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit('/').next())
            .and_then(|total| total.parse().ok())
            .ok_or_else(|| io::Error::other(format!("{url}: no range support")))?;
        Ok(Self {
            agent: agent.clone(),
            url: url.to_owned(),
            len,
            cache: Mutex::new(Cache::default()),
        })
    }

    /// Return the length of the file.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Read exactly `buf.len()` bytes at `offset`.
    ///
    /// # Errors
    ///
    /// Returns an error if the range is not in the file or a request fails.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|&end| end <= self.len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "read past the end"))?;
        let mut pos = offset;
        while pos < end {
            let index = pos / BLOCK_SIZE;
            let block = self.block(index)?;
            let start = usize::try_from(pos - index * BLOCK_SIZE).map_err(io::Error::other)?;
            let take = block
                .len()
                .saturating_sub(start)
                .min(usize::try_from(end - pos).unwrap_or(usize::MAX));
            if take == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short block"));
            }
            let at = usize::try_from(pos - offset).map_err(io::Error::other)?;
            buf[at..at + take].copy_from_slice(&block[start..start + take]);
            pos += take as u64;
        }
        Ok(())
    }

    /// Return one block, from the cache or from the server.
    fn block(&self, index: u64) -> io::Result<Arc<[u8]>> {
        if let Some(block) = self.lock().blocks.get(&index) {
            return Ok(Arc::clone(block));
        }
        let start = index * BLOCK_SIZE;
        let last = (start + BLOCK_SIZE).min(self.len) - 1;
        let mut response = self
            .agent
            .get(&self.url)
            .header("Range", &format!("bytes={start}-{last}"))
            .call()
            .map_err(io::Error::other)?;
        let block: Arc<[u8]> = response
            .body_mut()
            .with_config()
            // The limit must be larger than the block: ureq rejects a body of the limit size.
            .limit(BLOCK_SIZE + 1)
            .read_to_vec()
            .map_err(io::Error::other)?
            .into();
        let mut cache = self.lock();
        if cache.order.len() >= CACHE_BLOCKS
            && let Some(old) = cache.order.pop_front()
        {
            cache.blocks.remove(&old);
        }
        cache.order.push_back(index);
        cache.blocks.insert(index, Arc::clone(&block));
        Ok(block)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
