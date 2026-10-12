//! An OPFS-backed store (`wasm32`): a project in the browser's Origin Private
//! File System, so the editor can open, edit and save one in a browser.
//!
//! OPFS itself is asynchronous, but [`Store`] is synchronous. So this keeps a
//! **working copy** in memory: [`OpfsStore::open`] reads the OPFS tree into it, the
//! `Store` methods edit the copy, and [`OpfsStore::save`] writes it back. See
//! `.notes/web.md`.

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
        let root = opfs_root().await?;
        let dir = get_directory(&root, name).await?;
        let mut cache = MemoryStore::new();
        read_into(&dir, "", &mut cache).await?;
        Ok(Self { dir, cache })
    }

    /// Write the working copy back to OPFS.
    pub async fn save(&self) -> Result<(), StoreError> {
        for (path, bytes) in self.cache.files() {
            let (parent, name) = split(path);
            let dir = self.subdirectory(parent).await?;
            let handle = JsFuture::from(dir.get_file_handle(name))
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

    /// Get (creating as needed) the nested OPFS directory for a `/`-joined path.
    async fn subdirectory(&self, parent: &str) -> Result<FileSystemDirectoryHandle, StoreError> {
        let mut dir = self.dir.clone();
        for part in parent.split('/').filter(|part| !part.is_empty()) {
            dir = get_directory(&dir, part).await?;
        }
        Ok(dir)
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
    /// working copy (a `duplicate` is a detached view; only `save` from the owner
    /// persists).
    fn duplicate(&self) -> Box<dyn Store> {
        Box::new(OpfsStore {
            dir: self.dir.clone(),
            cache: self.cache.clone(),
        })
    }
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

/// Read `dir` (whose keys are prefixed by `prefix`) into `cache`, recursing.
async fn read_into(
    dir: &FileSystemDirectoryHandle,
    prefix: &str,
    cache: &mut MemoryStore,
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
            Box::pin(read_into(&child, &child_prefix, cache)).await?;
        } else {
            let file = JsFuture::from(handle.unchecked_into::<FileSystemFileHandle>().get_file())
                .await
                .map_err(js_err)?
                .dyn_into::<web_sys::File>()
                .map_err(|_| invalid("OPFS: an entry is not a file"))?;
            let buffer = JsFuture::from(file.array_buffer()).await.map_err(js_err)?;
            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            cache.write(&format!("{prefix}{name}"), &bytes)?;
        }
    }
    Ok(())
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
