//! An OPFS-backed store (`wasm32`): a project in the browser's Origin Private
//! File System, so the editor can open, edit and save one in a browser.
//!
//! OPFS itself is asynchronous, but [`Store`] is synchronous. So this keeps a
//! **working copy** in memory: [`OpfsStore::open`] reads the OPFS tree into it, the
//! `Store` methods edit the copy, and [`OpfsStore::save`] writes it back — the
//! whole-tree helpers [`load_tree`]/[`save_tree`] let a host persist a store it
//! does not own (see `.notes/web.md`).

use std::collections::BTreeMap;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemHandle, FileSystemHandleKind,
};

use crate::{Entry, MemoryStore, Store, StoreError};

/// A [`Store`] over a project in the browser's Origin Private File System.
///
/// See the module docs: it edits an in-memory working copy, and
/// [`OpfsStore::save`] persists it.
#[derive(Debug)]
pub struct OpfsStore {
    /// The OPFS directory the working copy was read from and is saved back to.
    dir: FileSystemDirectoryHandle,
    /// The working copy the synchronous [`Store`] methods edit.
    cache: MemoryStore,
}

impl OpfsStore {
    /// Open (creating if absent) the OPFS directory `name` as a store.
    pub async fn open(name: &str) -> Result<Self, StoreError> {
        let dir = directory(name).await?;
        let cache = MemoryStore::from_files(read_into(&dir, "").await?);
        Ok(Self { dir, cache })
    }

    /// Write the working copy back to OPFS.
    pub async fn save(&self) -> Result<(), StoreError> {
        write_into(&self.dir, self.cache.files()).await
    }
}

impl Store for OpfsStore {
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError> {
        self.cache.read(path)
    }

    fn read_capped(&self, path: &str, max: u64) -> Result<Vec<u8>, StoreError> {
        self.cache.read_capped(path, max)
    }

    fn read_string(&self, path: &str) -> Result<String, StoreError> {
        self.cache.read_string(path)
    }

    fn exists(&self, path: &str) -> bool {
        self.cache.exists(path)
    }

    fn is_dir(&self, path: &str) -> bool {
        self.cache.is_dir(path)
    }

    fn list(&self, dir: &str) -> Result<Vec<Entry>, StoreError> {
        self.cache.list(dir)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.cache.write(path, bytes)
    }

    fn remove(&mut self, path: &str) -> Result<(), StoreError> {
        self.cache.remove(path)
    }

    /// A second store over the same OPFS directory, holding a snapshot copy of the
    /// working copy (a detached view; only the owner's `save` persists).
    fn duplicate(&self) -> Box<dyn Store> {
        Box::new(OpfsStore {
            dir: self.dir.clone(),
            cache: self.cache.clone(),
        })
    }
}

/// Read the OPFS directory `name` into a flat tree (`/`-joined path to bytes).
pub async fn load_tree(name: &str) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let dir = directory(name).await?;
    read_into(&dir, "").await
}

/// Write a flat tree into the OPFS directory `name` (creating it if absent).
pub async fn save_tree(name: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<(), StoreError> {
    let dir = directory(name).await?;
    write_into(&dir, files).await
}

/// The OPFS directory `name`, creating it if absent.
async fn directory(name: &str) -> Result<FileSystemDirectoryHandle, StoreError> {
    let root = opfs_root().await?;
    get_directory(&root, name).await
}

/// The OPFS root directory (`navigator.storage.getDirectory()`).
async fn opfs_root() -> Result<FileSystemDirectoryHandle, StoreError> {
    let storage = web_sys::window()
        .ok_or_else(|| invalid("OPFS: no window"))?
        .navigator()
        .storage();
    JsFuture::from(storage.get_directory())
        .await
        .map_err(js_err)?
        .dyn_into::<FileSystemDirectoryHandle>()
        .map_err(|_| invalid("OPFS: getDirectory did not return a directory"))
}

/// Get (creating if absent) the child directory `name` of `dir`.
async fn get_directory(
    dir: &FileSystemDirectoryHandle,
    name: &str,
) -> Result<FileSystemDirectoryHandle, StoreError> {
    JsFuture::from(dir.get_directory_handle(name))
        .await
        .map_err(js_err)?
        .dyn_into::<FileSystemDirectoryHandle>()
        .map_err(|_| invalid(format!("OPFS: {name:?} is not a directory")))
}

/// Read `dir` (whose keys are prefixed by `prefix`) into a flat tree.
async fn read_into(
    dir: &FileSystemDirectoryHandle,
    prefix: &str,
) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let mut files = BTreeMap::new();
    read_dir(dir, prefix, &mut files).await?;
    Ok(files)
}

/// Recursively collect `dir` into `files`.
async fn read_dir(
    dir: &FileSystemDirectoryHandle,
    prefix: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    let iterator = web_sys::FileSystemDirectoryHandle::values(dir);
    loop {
        let next = JsFuture::from(iterator.next().map_err(js_err)?)
            .await
            .map_err(js_err)?;
        if property(&next, "done")?.as_bool().unwrap_or(false) {
            break;
        }
        let handle = property(&next, "value")?
            .dyn_into::<FileSystemHandle>()
            .map_err(|_| invalid("OPFS: an entry is not a file or a directory"))?;
        let name = handle.name();
        if handle.kind() == FileSystemHandleKind::Directory {
            let child = handle.unchecked_into::<FileSystemDirectoryHandle>();
            let child_prefix = format!("{prefix}{name}/");
            Box::pin(read_dir(&child, &child_prefix, files)).await?;
        } else {
            let file = JsFuture::from(handle.unchecked_into::<FileSystemFileHandle>().get_file())
                .await
                .map_err(js_err)?
                .dyn_into::<web_sys::File>()
                .map_err(|_| invalid("OPFS: an entry is not a file"))?;
            let buffer = JsFuture::from(file.array_buffer()).await.map_err(js_err)?;
            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            files.insert(format!("{prefix}{name}"), bytes);
        }
    }
    Ok(())
}

/// Write `files` under `dir`, creating subdirectories as needed.
async fn write_into(
    dir: &FileSystemDirectoryHandle,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    for (path, bytes) in files {
        let (parent, name) = split(path);
        let target = subdirectory(dir, parent).await?;
        let handle = JsFuture::from(target.get_file_handle(name))
            .await
            .map_err(js_err)?
            .dyn_into::<FileSystemFileHandle>()
            .map_err(|_| invalid(format!("OPFS: {path:?} is not a file")))?;
        let writable = JsFuture::from(handle.create_writable())
            .await
            .map_err(js_err)?;
        let stream = writable
            .dyn_into::<web_sys::FileSystemWritableFileStream>()
            .map_err(|_| invalid("OPFS: no writable stream"))?;
        JsFuture::from(stream.write_with_u8_array(bytes).map_err(js_err)?)
            .await
            .map_err(js_err)?;
        // `close` lives on the parent `WritableStream`.
        let stream: &web_sys::WritableStream = stream.unchecked_ref();
        JsFuture::from(web_sys::WritableStream::close(stream))
            .await
            .map_err(js_err)?;
    }
    Ok(())
}

/// Get (creating as needed) the nested directory of a `/`-joined path under `dir`.
async fn subdirectory(
    dir: &FileSystemDirectoryHandle,
    parent: &str,
) -> Result<FileSystemDirectoryHandle, StoreError> {
    let mut dir = dir.clone();
    for part in parent.split('/').filter(|part| !part.is_empty()) {
        dir = get_directory(&dir, part).await?;
    }
    Ok(dir)
}

/// Split a `/`-joined path into its parent directory and file name.
fn split(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((parent, name)) => (parent, name),
        None => ("", path),
    }
}

/// Read a property of a JS object.
fn property(value: &JsValue, name: &str) -> Result<JsValue, StoreError> {
    js_sys::Reflect::get(value, &JsValue::from_str(name)).map_err(js_err)
}

/// A `StoreError::Invalid` for a browser-storage failure.
fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::Invalid(message.into())
}

/// A thrown JS value as a `StoreError::Io`.
fn js_err(error: JsValue) -> StoreError {
    StoreError::Io(format!("OPFS: {}", describe(&error)))
}

/// A human-readable rendering of a thrown JS value.
fn describe(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(error, &JsValue::from_str("message"))
                .ok()
                .and_then(|message| message.as_string())
        })
        .unwrap_or_else(|| format!("{error:?}"))
}
