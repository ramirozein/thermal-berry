use crate::{readings::now_ms, storage::data_directory};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::Path;

#[derive(Debug)]
pub enum LockError {
    Busy(String),
    Io(io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(owner) => write!(f, "Thermal Berry ya está en ejecución ({owner})"),
            Self::Io(e) => write!(f, "control.lock: {e}"),
        }
    }
}

impl std::error::Error for LockError {}
impl From<io::Error> for LockError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub struct InstanceLock {
    file: File,
}

impl InstanceLock {
    pub fn acquire(mode: &str) -> Result<Self, LockError> {
        Self::in_directory(&data_directory()?, mode)
    }

    fn in_directory(dir: &Path, mode: &str) -> Result<Self, LockError> {
        let mut file = open(dir)?;
        if !try_lock(&file)? {
            let mut owner = String::new();
            if (&file).take(128).read_to_string(&mut owner).is_err() || owner.trim().is_empty() {
                owner = "otra instancia está iniciando".into();
            }
            return Err(LockError::Busy(owner.trim().to_string()));
        }
        file.set_len(0)?;
        file.rewind()?;
        writeln!(
            file,
            "pid {}, modo {}, inicio {}",
            std::process::id(),
            mode,
            now_ms()
        )?;
        Ok(Self { file })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Clear the departing owner while still locked. Keeping the inode
        // stable prevents a concurrent opener from bypassing the lock.
        let _ = self.file.set_len(0);
    }
}

#[cfg(unix)]
fn open(dir: &Path) -> io::Result<File> {
    use crate::storage::unix::{private_directory, set_private_mode};
    use std::os::unix::fs::OpenOptionsExt;
    let uid = private_directory(dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("control.lock"))?;
    set_private_mode(&file, uid, 0o600, false)?;
    Ok(file)
}

#[cfg(unix)]
fn try_lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(windows)]
fn open(dir: &Path) -> io::Result<File> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    std::fs::create_dir_all(dir)?;
    if dir.symlink_metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("unsafe control lock directory"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(dir.join("control.lock"))?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file is live and info points to a writable C structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.nNumberOfLinks != 1
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !file.metadata()?.is_file()
    {
        return Err(io::Error::other("unsafe control lock file"));
    }
    Ok(file)
}

#[cfg(windows)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::ERROR_LOCK_VIOLATION,
        Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY},
        System::IO::OVERLAPPED,
    };
    // Windows makes a locked byte range unreadable to other processes. Place
    // the lock beyond the owner record, which must remain readable on contention.
    let mut offset = OVERLAPPED::default();
    offset.Anonymous.Anonymous.Offset = 256;
    // SAFETY: file owns the synchronous handle and offset lives through the call.
    if unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut offset,
        )
    } != 0
    {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(e)
    }
}
