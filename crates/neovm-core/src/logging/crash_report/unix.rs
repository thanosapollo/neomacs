//! Descriptor-relative, no-follow creation: never append to a shared pathname.

use super::*;
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Component;

pub(super) struct Destination {
    directory: File,
    name: CString,
    path: PathBuf,
}

fn owned_fd(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: each caller passes a new successful open/openat descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unsafe crash report directory",
    )
}

impl Destination {
    pub(super) fn new(path: &Path, pid: u32, started: u128) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(refused());
        }
        // SAFETY: constant nul-terminated pathname, no pointer retained.
        let mut directory = owned_fd(unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        for component in path.components() {
            let Component::Normal(component) = component else {
                if component == Component::RootDir {
                    continue;
                }
                return Err(refused());
            };
            let name = CString::new(component.as_bytes()).map_err(|_| refused())?;
            // SAFETY: live directory fd and nul-terminated single component.
            let result = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: same live directory/name. NOFOLLOW rejects symlinks;
            // DIRECTORY prevents special files; CLOEXEC prevents child leakage.
            directory = owned_fd(unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })?;
            let metadata = directory.metadata()?;
            // Root-owned sticky shared ancestors (e.g. /tmp) are safe to walk:
            // another uid cannot replace our owned child. The final directory
            // below must still be ours and private. Reject other writable or
            // foreign-owned ancestors, not just an unsafe final component.
            let sticky_root = metadata.uid() == 0 && metadata.mode() & libc::S_ISVTX != 0;
            if (metadata.uid() != uid && metadata.uid() != 0)
                || (metadata.mode() & 0o022 != 0 && !sticky_root)
            {
                return Err(refused());
            }
        }
        let metadata = directory.metadata()?;
        if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(refused());
        }
        let name = format!("panic-{started}-{pid}.txt");
        Ok(Self {
            directory,
            path: path.join(&name),
            name: CString::new(name).map_err(|_| refused())?,
        })
    }

    pub(super) fn create(&self) -> io::Result<File> {
        // SAFETY: live directory fd and owned nul-terminated filename. EXCL
        // refuses every preexisting entry, including a symlink or hard link.
        // The retained directory fd avoids pathname substitution after setup.
        owned_fd(unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        })
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        self.directory.sync_all()
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}
