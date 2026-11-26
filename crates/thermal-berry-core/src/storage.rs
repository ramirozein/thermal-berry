use std::{io, path::PathBuf};

pub(crate) fn data_directory() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let base = dirs::data_local_dir();
    #[cfg(not(windows))]
    let base = dirs::data_dir();
    base.map(|p| p.join("thermal-berry"))
        .ok_or_else(|| io::Error::other("no data directory available"))
}

#[cfg(unix)]
pub(crate) mod unix {
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub(crate) fn private_directory(directory: &Path) -> std::io::Result<u32> {
        fs::create_dir_all(
            directory
                .parent()
                .expect("application directory has a parent"),
        )?;
        match DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }

        // SAFETY: geteuid takes no arguments and returns the current effective UID.
        let uid = unsafe { libc::geteuid() };
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(directory)?;
        set_private_mode(&dir, uid, 0o700, true)?;

        Ok(uid)
    }

    pub(crate) fn set_private_mode(
        file: &File,
        uid: u32,
        mode: u32,
        directory: bool,
    ) -> std::io::Result<()> {
        let metadata = file.metadata()?;
        if metadata.uid() != uid
            || (directory && !metadata.is_dir())
            || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "unsafe database path",
            ));
        }
        file.set_permissions(fs::Permissions::from_mode(mode))
    }
}
