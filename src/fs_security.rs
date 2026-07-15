use std::path::Path;

/// Create a directory intended to hold captured traffic and private key material.
pub fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }

    #[cfg(not(unix))]
    std::fs::create_dir_all(path)?;

    Ok(())
}

/// Atomically replace a sensitive file without following a destination symlink.
pub fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    write_private_with(path, |file| {
        use std::io::Write;
        file.write_all(contents.as_ref())
    })
}

pub fn write_private_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if parent.exists() {
            if !parent.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    format!("{} is not a directory", parent.display()),
                ));
            }
        } else {
            ensure_private_dir(parent)?;
        }
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("private");
    let mut attempt = 0u32;
    let (temporary, mut file) = loop {
        let candidate = path.with_file_name(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt = attempt.checked_add(1).ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    };

    let result = (|| {
        write(&mut file)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        set_private_file_permissions(path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn harden_private_tree(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return ensure_private_dir(path);
    }
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing symlinked private directory {}", path.display()),
        ));
    }
    harden_entry(path)
}

fn harden_entry(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        ensure_private_dir(path)?;
        for entry in std::fs::read_dir(path)? {
            harden_entry(&entry?.path())?;
        }
    } else if metadata.is_file() {
        set_private_file_permissions(path)?;
    }
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "crowbar-{name}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[cfg(unix)]
    #[test]
    fn private_files_and_directories_have_restricted_modes() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root("private-files");
        let file = root.join("nested/session.json");
        write_private(&file, b"secret").unwrap();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(root.join("nested"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn harden_private_tree_repairs_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root("harden-tree");
        let nested = root.join("sessions");
        let file = nested.join("session.json");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&file, b"captured credentials").unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

        harden_private_tree(&root).unwrap();

        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_write_replaces_symlink_without_overwriting_target() {
        use std::os::unix::fs::symlink;

        let root = test_root("symlink-write");
        let target = root.join("outside.txt");
        let private = root.join("ca.key");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&target, b"do not replace").unwrap();
        symlink(&target, &private).unwrap();

        write_private(&private, b"private key").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"do not replace");
        assert_eq!(std::fs::read(&private).unwrap(), b"private key");
        assert!(
            !std::fs::symlink_metadata(&private)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
