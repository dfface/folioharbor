use std::{
    ffi::OsString,
    io,
    path::{Component, Path},
};

use cap_fs_ext::DirExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, DirBuilder},
};

pub(crate) struct SecureRoot {
    dir: Dir,
}

impl SecureRoot {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        if absolute.parent().is_none() {
            return Dir::open_ambient_dir(&absolute, ambient_authority()).map(|dir| Self { dir });
        }

        let mut base = absolute.as_path();
        while !base.exists() {
            base = base.parent().ok_or_else(invalid_path)?;
        }
        if base == absolute {
            base = absolute.parent().ok_or_else(invalid_path)?;
        }
        let relative = absolute.strip_prefix(base).map_err(|_| invalid_path())?;
        let mut dir = Dir::open_ambient_dir(base, ambient_authority())?;
        for component in relative.components() {
            let name = normal_component(component)?;
            dir = open_or_create_private(&dir, Path::new(&name))?;
        }
        Ok(Self { dir })
    }

    pub(crate) fn open_parent(&self, relative: &Path, create: bool) -> io::Result<(Dir, OsString)> {
        let mut components = relative.components().peekable();
        let mut dir = self.dir.try_clone()?;
        while let Some(component) = components.next() {
            let name = normal_component(component)?;
            if components.peek().is_none() {
                return Ok((dir, name));
            }
            dir = if create {
                open_or_create_private(&dir, Path::new(&name))?
            } else {
                dir.open_dir_nofollow(Path::new(&name))?
            };
        }
        Err(invalid_path())
    }

    pub(crate) fn open_dir(&self, relative: &Path, create: bool) -> io::Result<Dir> {
        let mut dir = self.dir.try_clone()?;
        for component in relative.components() {
            let name = normal_component(component)?;
            dir = if create {
                open_or_create_private(&dir, Path::new(&name))?
            } else {
                dir.open_dir_nofollow(Path::new(&name))?
            };
        }
        Ok(dir)
    }

    pub(crate) fn verify_private(&self) -> io::Result<()> {
        verify_private_dir(&self.dir)
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        sync_dir(&self.dir)
    }
}

pub(crate) fn verify_private_dir(dir: &Dir) -> io::Result<()> {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt as _;
        if dir.dir_metadata()?.mode() & 0o777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "storage capability directory is not private",
            ));
        }
    }
    Ok(())
}

pub(crate) fn sync_dir(dir: &Dir) -> io::Result<()> {
    dir.try_clone()?.into_std_file().sync_all()
}

fn open_or_create_private(parent: &Dir, name: &Path) -> io::Result<Dir> {
    match parent.open_dir_nofollow(name) {
        Ok(dir) => Ok(dir),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use cap_std::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match parent.create_dir_with(name, &builder) {
                Ok(()) => sync_dir(parent)?,
                Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(create_error) => return Err(create_error),
            }
            parent.open_dir_nofollow(name)
        }
        Err(error) => Err(error),
    }
}

fn normal_component(component: Component<'_>) -> io::Result<OsString> {
    match component {
        Component::Normal(value) => Ok(value.to_owned()),
        _ => Err(invalid_path()),
    }
}

fn invalid_path() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid capability path")
}
