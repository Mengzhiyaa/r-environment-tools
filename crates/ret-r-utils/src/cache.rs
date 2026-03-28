// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use lazy_static::lazy_static;
use log::trace;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use crate::{
    env::ResolvedRInstallation,
    fs_cache::{delete_cache_file, get_cache_from_file, store_cache_in_file},
};

lazy_static! {
    static ref CACHE: CacheImpl = CacheImpl::new(None);
}

pub trait CacheEntry: Send + Sync {
    fn get(&self) -> Option<ResolvedRInstallation>;
    fn store(&self, installation: ResolvedRInstallation);
    fn track_symlinks(&self, symlinks: Vec<PathBuf>);
}

pub fn clear_cache() -> io::Result<()> {
    CACHE.clear()
}

pub fn create_cache(executable: PathBuf) -> Arc<Mutex<Box<dyn CacheEntry>>> {
    CACHE.create_cache(executable)
}

pub fn get_cache_directory() -> Option<PathBuf> {
    CACHE.get_cache_directory()
}

pub fn set_cache_directory(cache_dir: PathBuf) {
    CACHE.set_cache_directory(cache_dir)
}

pub type LockableCacheEntry = Arc<Mutex<Box<dyn CacheEntry>>>;

struct CacheImpl {
    cache_dir: Arc<Mutex<Option<PathBuf>>>,
    locks: Mutex<HashMap<PathBuf, LockableCacheEntry>>,
}

impl CacheImpl {
    fn new(cache_dir: Option<PathBuf>) -> CacheImpl {
        CacheImpl {
            cache_dir: Arc::new(Mutex::new(cache_dir)),
            locks: Mutex::new(HashMap::<PathBuf, LockableCacheEntry>::new()),
        }
    }

    fn get_cache_directory(&self) -> Option<PathBuf> {
        self.cache_dir
            .lock()
            .expect("cache_dir mutex poisoned")
            .clone()
    }

    fn set_cache_directory(&self, cache_dir: PathBuf) {
        trace!("Setting cache directory to {:?}", cache_dir);
        let old = self
            .cache_dir
            .lock()
            .expect("cache_dir mutex poisoned")
            .replace(cache_dir.clone());
        if old.as_ref() != Some(&cache_dir) {
            // Directory changed: clear existing entries so subsequent
            // create_cache() calls pick up the new directory.
            self.locks.lock().expect("locks mutex poisoned").clear();
        }
    }

    fn clear(&self) -> io::Result<()> {
        trace!("Clearing cache");
        self.locks.lock().expect("locks mutex poisoned").clear();
        if let Some(cache_directory) = self
            .cache_dir
            .lock()
            .expect("cache_dir mutex poisoned")
            .clone()
        {
            std::fs::remove_dir_all(cache_directory)
        } else {
            Ok(())
        }
    }

    fn create_cache(&self, executable: PathBuf) -> LockableCacheEntry {
        let cache_directory = self
            .cache_dir
            .lock()
            .expect("cache_dir mutex poisoned")
            .clone();
        match self
            .locks
            .lock()
            .expect("locks mutex poisoned")
            .entry(executable.clone())
        {
            Entry::Occupied(lock) => lock.get().clone(),
            Entry::Vacant(lock) => {
                let cache = Box::new(CacheEntryImpl::create(cache_directory.clone(), executable))
                    as Box<dyn CacheEntry + 'static>;
                lock.insert(Arc::new(Mutex::new(cache))).clone()
            }
        }
    }
}

type FilePathWithMTimeCTime = (PathBuf, SystemTime, Option<SystemTime>);

struct CacheEntryImpl {
    cache_directory: Option<PathBuf>,
    executable: PathBuf,
    installation: Arc<Mutex<Option<ResolvedRInstallation>>>,
    symlinks: Arc<Mutex<Vec<FilePathWithMTimeCTime>>>,
}

impl CacheEntryImpl {
    pub fn create(cache_directory: Option<PathBuf>, executable: PathBuf) -> impl CacheEntry {
        CacheEntryImpl {
            cache_directory,
            executable,
            installation: Arc::new(Mutex::new(None)),
            symlinks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn verify_in_memory_cache(&self) {
        for symlink_info in self
            .symlinks
            .lock()
            .expect("symlinks mutex poisoned")
            .iter()
        {
            if let Ok(metadata) = symlink_info.0.metadata() {
                let mtime_changed = metadata.modified().ok() != Some(symlink_info.1);
                let ctime_changed = match symlink_info.2 {
                    Some(stored_ctime) => metadata.created().ok() != Some(stored_ctime),
                    None => false,
                };
                if mtime_changed || ctime_changed {
                    trace!("Symlink {:?} changed since it was cached", symlink_info.0);
                    self.installation
                        .lock()
                        .expect("installation mutex poisoned")
                        .take();
                    if let Some(cache_directory) = &self.cache_directory {
                        delete_cache_file(cache_directory, &self.executable);
                    }
                }
            }
        }
    }
}

impl CacheEntry for CacheEntryImpl {
    fn get(&self) -> Option<ResolvedRInstallation> {
        self.verify_in_memory_cache();

        {
            if let Some(installation) = self
                .installation
                .lock()
                .expect("installation mutex poisoned")
                .clone()
            {
                return Some(installation);
            }
        }

        if let Some(ref cache_directory) = self.cache_directory {
            let (installation, mut symlinks) =
                get_cache_from_file(cache_directory, &self.executable)?;
            self.installation
                .lock()
                .expect("installation mutex poisoned")
                .replace(installation.clone());
            let mut locked_symlinks = self.symlinks.lock().expect("symlinks mutex poisoned");
            locked_symlinks.clear();
            locked_symlinks.append(&mut symlinks);
            Some(installation)
        } else {
            None
        }
    }

    fn store(&self, installation: ResolvedRInstallation) {
        let mut symlinks = vec![];
        for symlink in installation.symlinks.clone().unwrap_or_default().iter() {
            if let Ok(metadata) = symlink.metadata() {
                if let Ok(modified) = metadata.modified() {
                    let created = metadata.created().ok();
                    symlinks.push((symlink.clone(), modified, created));
                }
            }
        }

        symlinks.sort();
        symlinks.dedup();

        {
            let mut locked_symlinks = self.symlinks.lock().expect("symlinks mutex poisoned");
            locked_symlinks.clear();
            locked_symlinks.append(&mut symlinks.clone());
        }
        self.installation
            .lock()
            .expect("installation mutex poisoned")
            .replace(installation.clone());

        trace!("Caching R installation info for {:?}", self.executable);

        if let Some(ref cache_directory) = self.cache_directory {
            store_cache_in_file(cache_directory, &self.executable, &installation, symlinks)
        }
    }

    fn track_symlinks(&self, symlinks: Vec<PathBuf>) {
        self.verify_in_memory_cache();

        let known_symlinks: HashSet<PathBuf> = self
            .symlinks
            .lock()
            .expect("symlinks mutex poisoned")
            .clone()
            .iter()
            .map(|x| x.0.clone())
            .collect();

        let symlinks_to_track = symlinks
            .into_iter()
            .filter(|path| !known_symlinks.contains(path))
            .filter_map(|path| {
                let metadata = path.metadata().ok()?;
                let modified = metadata.modified().ok()?;
                let created = metadata.created().ok();
                Some((path, modified, created))
            })
            .collect::<Vec<_>>();

        self.symlinks
            .lock()
            .expect("symlinks mutex poisoned")
            .extend(symlinks_to_track);
    }
}
