use forge_domain::ConversationId;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Private runtime directory shared by bridge and TUI. Never put it in /tmp.
pub fn runtime_dir() -> anyhow::Result<PathBuf> {
    let directory = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("home unavailable"))?
        .join(".forge")
        .join("live");
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = std::fs::symlink_metadata(&directory)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink() && metadata.is_dir(),
            "unsafe runtime directory"
        );
        // SAFETY: geteuid has no preconditions and does not mutate process state.
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "runtime directory owner mismatch"
        );
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

/// An OS lock survives stale files and releases only when the runtime exits.
pub fn acquire(directory: &Path, session: ConversationId) -> anyhow::Result<File> {
    let path = directory.join(format!("{session}.lock"));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    file.try_lock()
        .map_err(|error| anyhow::anyhow!("session already owned or lease unavailable: {error}"))?;
    Ok(file)
}

pub fn socket_path(directory: &Path, session: ConversationId) -> PathBuf {
    directory.join(format!("{session}.sock"))
}
