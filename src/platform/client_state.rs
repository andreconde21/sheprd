use std::path::Path;

#[cfg(not(windows))]
pub(crate) fn create_private_state_file(path: &Path) -> std::io::Result<std::fs::File> {
    super::create_remote_ssh_config_file(path)
}

#[cfg(windows)]
pub(crate) fn create_private_state_file(path: &Path) -> std::io::Result<std::fs::File> {
    super::windows::create_remote_ssh_config_file(path)
}

#[cfg(not(windows))]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    super::windows::replace_file(source, destination)
}

#[cfg(not(windows))]
pub(crate) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
pub(crate) fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    // replace_file uses MOVEFILE_WRITE_THROUGH on Windows.
    Ok(())
}

/// andreconde fork (sheprd): gives `file` the owner of `like` (SHE-100010), so replacing a
/// user's file from a server running as root does not hand it to root. Fails without the
/// privilege to change owners; callers treat it as best effort.
#[cfg(unix)]
pub(crate) fn copy_file_owner(
    like: &std::fs::Metadata,
    file: &std::fs::File,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    std::os::unix::fs::fchown(file, Some(like.uid()), Some(like.gid()))
}

/// Files have no Unix owner to keep here.
#[cfg(not(unix))]
pub(crate) fn copy_file_owner(
    _like: &std::fs::Metadata,
    _file: &std::fs::File,
) -> std::io::Result<()> {
    Ok(())
}
