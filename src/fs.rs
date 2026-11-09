//! First-party filesystem effects.
//!
//! Provisional. Every descriptor names a path relative to a root the host
//! chooses when it binds [`FsDriver`]; a Component never sees an absolute
//! path. Each descriptor is one finite, atomic interaction: there are no open
//! or close effects, and [`ReplaceFile`] performs its temp-write-rename sequence
//! inside one effect so partial states cannot leak into a Model. See
//! `docs/notes/filesystem-design.md` for the reasoning and open questions.

use std::error::Error;
use std::fmt;
use std::path::{Component as PathComponent, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;

use crate::{BoxFuture, EffectDescriptor, EffectDriver};

/// Appends bytes to a file under the bound root.
///
/// The file and any missing parent directories are created. One append is one
/// write; two appends from the same Component have no promised order yet, so a
/// Component that needs order sequences through outcomes.
///
/// ```
/// use samara::AppendToFile;
/// let append = AppendToFile::line("journal.log", "started");
/// assert_eq!(append.path(), std::path::Path::new("journal.log"));
/// assert_eq!(append.as_bytes(), b"started\n");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendToFile {
    path: PathBuf,
    bytes: Bytes,
}

impl AppendToFile {
    /// Appends exactly these bytes.
    pub fn bytes(path: impl Into<PathBuf>, bytes: impl Into<Bytes>) -> Self {
        Self {
            path: path.into(),
            bytes: bytes.into(),
        }
    }

    /// Appends exactly this text with no added newline.
    pub fn text(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        Self::bytes(path, text.into())
    }

    /// Appends this text followed by one newline.
    pub fn line(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        let mut text = text.into();
        text.push('\n');
        Self::text(path, text)
    }

    /// Returns the root-relative path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the bytes that will be appended.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the bytes as text when they are valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

impl EffectDescriptor for AppendToFile {
    type Output = ();
    type Error = FsError;
}

/// Replaces a file's whole contents atomically.
///
/// The driver writes a temporary sibling file, syncs it, and renames it over
/// the target, so a reader sees either the old contents or the new ones.
/// Missing parent directories are created. The directory entry itself is not
/// synced.
///
/// ```
/// use samara::ReplaceFile;
/// let replace = ReplaceFile::text("state.json", "{}");
/// assert_eq!(replace.as_bytes(), b"{}");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaceFile {
    path: PathBuf,
    bytes: Bytes,
}

impl ReplaceFile {
    /// Replaces the contents with exactly these bytes.
    pub fn bytes(path: impl Into<PathBuf>, bytes: impl Into<Bytes>) -> Self {
        Self {
            path: path.into(),
            bytes: bytes.into(),
        }
    }

    /// Replaces the contents with exactly this text.
    pub fn text(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        Self::bytes(path, text.into())
    }

    /// Returns the root-relative path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the bytes that will be written.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the bytes as text when they are valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

impl EffectDescriptor for ReplaceFile {
    type Output = ();
    type Error = FsError;
}

/// Reads a whole file under the bound root.
///
/// ```
/// use samara::ReadFile;
/// assert_eq!(ReadFile::new("state.json").path(), std::path::Path::new("state.json"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFile {
    path: PathBuf,
}

impl ReadFile {
    /// Reads the file at this root-relative path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the root-relative path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EffectDescriptor for ReadFile {
    type Output = Bytes;
    type Error = FsError;
}

/// Removes a file under the bound root.
///
/// Removing a file that does not exist fails with [`FsErrorKind::NotFound`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveFile {
    path: PathBuf,
}

impl RemoveFile {
    /// Removes the file at this root-relative path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the root-relative path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EffectDescriptor for RemoveFile {
    type Output = ();
    type Error = FsError;
}

/// Which filesystem effect an [`FsError`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FsOperation {
    /// [`AppendToFile`].
    Append,
    /// [`ReplaceFile`].
    Replace,
    /// [`ReadFile`].
    Read,
    /// [`RemoveFile`].
    Remove,
}

/// Why a filesystem effect failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FsErrorKind {
    /// The path was absolute or escaped the bound root with `..`.
    OutsideRoot,
    /// The file or a parent directory does not exist.
    NotFound,
    /// The process may not perform this operation on the path.
    PermissionDenied,
    /// Any other I/O failure; see the message.
    Io,
}

/// A filesystem failure naming the operation, the root-relative path, and a category.
///
/// ```
/// use samara::{FsError, FsErrorKind, FsOperation};
/// let error = FsError::new(FsOperation::Read, "missing.txt", FsErrorKind::NotFound, "no such file");
/// assert_eq!(error.kind(), FsErrorKind::NotFound);
/// assert_eq!(error.operation(), FsOperation::Read);
/// assert_eq!(error.to_string(), "filesystem Read of missing.txt failed (NotFound): no such file");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsError {
    operation: FsOperation,
    path: PathBuf,
    kind: FsErrorKind,
    message: Arc<str>,
}

impl FsError {
    /// Creates a filesystem error, for example to supply a failure in a controlled test.
    pub fn new(
        operation: FsOperation,
        path: impl Into<PathBuf>,
        kind: FsErrorKind,
        message: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            operation,
            path: path.into(),
            kind,
            message: message.into(),
        }
    }

    fn io(operation: FsOperation, path: &Path, error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => FsErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => FsErrorKind::PermissionDenied,
            _ => FsErrorKind::Io,
        };
        Self::new(operation, path, kind, error.to_string())
    }

    fn outside_root(operation: FsOperation, path: &Path) -> Self {
        Self::new(
            operation,
            path,
            FsErrorKind::OutsideRoot,
            "path must be relative and must not contain `..`",
        )
    }

    /// Returns the effect that failed.
    pub fn operation(&self) -> FsOperation {
        self.operation
    }

    /// Returns the root-relative path the effect named.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the error category.
    pub fn kind(&self) -> FsErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "filesystem {:?} of {} failed ({:?}): {}",
            self.operation,
            self.path.display(),
            self.kind,
            self.message
        )
    }
}

impl Error for FsError {}

/// Live Tokio driver for the filesystem effects, confined to one root directory.
///
/// Every path in a descriptor is joined to the root. Absolute paths and paths
/// containing `..` fail with [`FsErrorKind::OutsideRoot`] before any I/O. The
/// root itself is not created until the first write needs it. Bind all four
/// effects at once with [`LiveRuntimeBuilder::bind_fs`](crate::LiveRuntimeBuilder::bind_fs),
/// or bind one driver per effect when a Subprogram's `Drivers` asks for them.
#[derive(Clone, Debug)]
pub struct FsDriver {
    root: Arc<Path>,
}

static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

impl FsDriver {
    /// Creates a driver that resolves every descriptor path under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Arc::from(root.into()),
        }
    }

    /// Returns the root every path is resolved against.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, operation: FsOperation, path: &Path) -> Result<PathBuf, FsError> {
        let confined = path
            .components()
            .all(|component| matches!(component, PathComponent::Normal(_) | PathComponent::CurDir));
        if !confined || path.as_os_str().is_empty() {
            return Err(FsError::outside_root(operation, path));
        }
        Ok(self.root.join(path))
    }
}

async fn ensure_parent(
    operation: FsOperation,
    relative: &Path,
    target: &Path,
) -> Result<(), FsError> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| FsError::io(operation, relative, error))?;
    }
    Ok(())
}

impl EffectDriver<AppendToFile> for FsDriver {
    fn execute(&self, descriptor: AppendToFile) -> BoxFuture<Result<(), FsError>> {
        let AppendToFile { path, bytes } = descriptor;
        let resolved = self.resolve(FsOperation::Append, &path);
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;
            let target = resolved?;
            ensure_parent(FsOperation::Append, &path, &target).await?;
            let io = |error| FsError::io(FsOperation::Append, &path, error);
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&target)
                .await
                .map_err(io)?;
            file.write_all(&bytes).await.map_err(io)?;
            file.flush().await.map_err(io)
        })
    }
}

impl EffectDriver<ReplaceFile> for FsDriver {
    fn execute(&self, descriptor: ReplaceFile) -> BoxFuture<Result<(), FsError>> {
        let ReplaceFile { path, bytes } = descriptor;
        let resolved = self.resolve(FsOperation::Replace, &path);
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;
            let target = resolved?;
            ensure_parent(FsOperation::Replace, &path, &target).await?;
            let io = |error| FsError::io(FsOperation::Replace, &path, error);
            let name = target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
            let temp = target.with_file_name(format!(".{name}.{}.{nonce}.tmp", std::process::id()));
            let written: Result<(), std::io::Error> = async {
                let mut file = tokio::fs::File::create(&temp).await?;
                file.write_all(&bytes).await?;
                file.sync_all().await?;
                drop(file);
                tokio::fs::rename(&temp, &target).await
            }
            .await;
            if let Err(error) = written {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(io(error));
            }
            Ok(())
        })
    }
}

impl EffectDriver<ReadFile> for FsDriver {
    fn execute(&self, descriptor: ReadFile) -> BoxFuture<Result<Bytes, FsError>> {
        let ReadFile { path } = descriptor;
        let resolved = self.resolve(FsOperation::Read, &path);
        Box::pin(async move {
            let target = resolved?;
            tokio::fs::read(&target)
                .await
                .map(Bytes::from)
                .map_err(|error| FsError::io(FsOperation::Read, &path, error))
        })
    }
}

impl EffectDriver<RemoveFile> for FsDriver {
    fn execute(&self, descriptor: RemoveFile) -> BoxFuture<Result<(), FsError>> {
        let RemoveFile { path } = descriptor;
        let resolved = self.resolve(FsOperation::Remove, &path);
        Box::pin(async move {
            let target = resolved?;
            tokio::fs::remove_file(&target)
                .await
                .map_err(|error| FsError::io(FsOperation::Remove, &path, error))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_outside_the_root_are_rejected_before_io() {
        let driver = FsDriver::new("/srv/app");
        for path in ["/etc/passwd", "../sibling", "a/../../b", ""] {
            let error = driver
                .resolve(FsOperation::Read, Path::new(path))
                .expect_err(path);
            assert_eq!(error.kind(), FsErrorKind::OutsideRoot, "{path}");
            assert_eq!(error.path(), Path::new(path));
        }
        assert_eq!(
            driver
                .resolve(FsOperation::Read, Path::new("./logs/app.log"))
                .unwrap(),
            PathBuf::from("/srv/app/./logs/app.log")
        );
    }
}
