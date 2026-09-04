//! `wl_shm` allocation helpers: memfd-backed shared memory pools and mmaps.

use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

use wayland_client::protocol::{wl_shm, wl_shm_pool};
use wayland_client::{Connection, QueueHandle};

use crate::session::Session;
use crate::BackendError;

/// An owned mmap region. Only ever accessed from the single backend thread,
/// hence the manual `Send`.
pub(crate) struct Mmap {
    ptr: *mut u8,
    len: usize,
}

// The mapping is owned exclusively by the pool object and is not aliased
// across threads.
unsafe impl Send for Mmap {}

impl Mmap {
    /// Maps `len` bytes of `file` read-write.
    pub fn map(file: &File, len: usize) -> Result<Self, BackendError> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(BackendError::Io(std::io::Error::last_os_error().to_string()));
        }
        Ok(Mmap { ptr: ptr as *mut u8, len })
    }

    /// Mutable view over the whole mapping.
    pub fn as_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    /// Immutable view over the whole mapping.
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

/// A live `wl_shm` pool with its backing memfd and mapping.
///
/// The file handle is kept open for the pool's lifetime: the protocol fd is
/// only transmitted on flush, so closing earlier would be unsafe.
pub(crate) struct ShmPool {
    pub pool: wl_shm_pool::WlShmPool,
    pub mmap: Mmap,
    _file: File,
    pub byte_size: usize,
}

impl ShmPool {
    /// Creates a memfd-backed pool of `byte_size` bytes.
    pub fn create(
        shm: &wl_shm::WlShm,
        byte_size: usize,
        qh: &QueueHandle<Session>,
    ) -> Result<Self, BackendError> {
        let name = std::ffi::CString::new("fidus-shm").unwrap();
        let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(BackendError::Io(std::io::Error::last_os_error().to_string()));
        }
        // SAFETY: `fd` is a freshly created owned descriptor.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        // The Wayland pool gets its own duplicate; the local handle backs the
        // mmap and keeps the file alive for the pool's lifetime.
        let pool_fd = owned
            .try_clone()
            .map_err(|e| BackendError::Io(e.to_string()))?;
        let file = File::from(owned);
        file.set_len(byte_size as u64)
            .map_err(|e| BackendError::Io(e.to_string()))?;
        let mmap = Mmap::map(&file, byte_size)?;
        let pool = shm.create_pool(pool_fd.as_fd(), byte_size as i32, qh, ());
        Ok(ShmPool { pool, mmap, _file: file, byte_size })
    }

    /// Destroys the protocol pool object (the mapping and memfd die with the
    /// struct).
    pub fn destroy(&self, _conn: &Connection) {
        self.pool.destroy();
    }
}
