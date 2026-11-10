//! A fixed file resource with compile-time read, append, and write permissions.
#[cfg(test)]
mod tests;
use crate::{
    AppendToFile, BoxFuture, CapabilityToken, Command, ControlledRuntime, EffectCapability,
    EffectDescriptor, EffectDriver, EffectOutcome, FsDriver, FsError, PendingEffect, ReadFile,
    ReplaceFile, RuntimeError,
};
use bytes::Bytes;
use std::{
    fmt,
    marker::PhantomData,
    path::{Path, PathBuf},
};

mod private {
    pub trait Sealed {}
}

/// A sealed set of operations available through a file capability.
pub trait FilePermissions: private::Sealed + Send + Sync + 'static {
    #[doc(hidden)]
    const READ: bool;
    #[doc(hidden)]
    const APPEND: bool;
    #[doc(hidden)]
    const WRITE: bool;
}
/// Permission to read a file without modifying it.
#[derive(Clone, Copy, Debug)]
pub struct ReadOnly;
/// Permission to append without reading, replacing, or truncating a file.
#[derive(Clone, Copy, Debug)]
pub struct AppendOnly;
/// Permission to read and append to the same file.
#[derive(Clone, Copy, Debug)]
pub struct ReadAppend;
/// Permission to replace or append contents without reading or removing the file.
#[derive(Clone, Copy, Debug)]
pub struct WriteOnly;
/// Permission to read, replace, and append contents without removing the file.
#[derive(Clone, Copy, Debug)]
pub struct ReadWrite;
macro_rules! permissions {
    ($name:ty, $read:literal, $append:literal, $write:literal) => {
        impl private::Sealed for $name {}
        impl FilePermissions for $name {
            const READ: bool = $read;
            const APPEND: bool = $append;
            const WRITE: bool = $write;
        }
    };
}
permissions!(ReadOnly, true, false, false);
permissions!(AppendOnly, false, true, false);
permissions!(ReadAppend, true, true, false);
permissions!(WriteOnly, false, true, true);
permissions!(ReadWrite, true, true, true);
/// Sealed permission sets that authorize reading.
pub trait CanRead: FilePermissions {}
/// Sealed permission sets that authorize appending.
pub trait CanAppend: FilePermissions {}
/// Sealed permission sets that authorize replacing contents as well as appending.
pub trait CanWrite: CanAppend {}
impl CanRead for ReadOnly {}
impl CanRead for ReadAppend {}
impl CanRead for ReadWrite {}
impl CanAppend for AppendOnly {}
impl CanAppend for ReadAppend {}
impl CanAppend for WriteOnly {}
impl CanAppend for ReadWrite {}
impl CanWrite for WriteOnly {}
impl CanWrite for ReadWrite {}

/// Declares one file resource with permissions `P` in a Program's requirements.
///
/// The host supplies one [`FileDriver`] for this position. Two positions of the
/// same type can bind different files. Construction and binding perform no I/O.
///
/// ```
/// use samara::prelude::*;
/// let (builder, (file,)) = Program::builder::<(File<ReadAppend>,)>();
/// let append: Command<()> = file.append_line("hello").discard_outcome();
/// assert_eq!(append.effect_intent::<FileAppend>().unwrap().as_bytes(), b"hello\n");
/// let runtime = Runtime::builder(builder.build()?)
///     .with_drivers((FileDriver::new("data/journal.log"),)).build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A file uses one binding, not independently supplied read and append Drivers:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (builder, _) = Program::builder::<File<ReadAppend>>();
/// Runtime::builder(builder.build().unwrap())
///     .with_drivers((FileDriver::new("a"), FileDriver::new("b")));
/// ```
pub struct File<P: FilePermissions>(PhantomData<fn() -> P>);

/// Inert authority to issue permitted operations against one logical file.
///
/// Issued by [`crate::Program::builder`]. Clones and narrowed views retain the
/// same resource identity. Each operation chooses data, never another target
/// path. The host binds the path once through [`FileDriver`].
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<ReadOnly>>();
/// file.append_line("not permitted");
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<AppendOnly>>();
/// file.read();
/// ```
///
/// Append authority does not permit replacement, even when reading is allowed:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<AppendOnly>>();
/// file.replace_text("not permitted");
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<ReadAppend>>();
/// file.replace_text("not permitted");
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<ReadOnly>>();
/// file.replace_bytes(vec![1, 2, 3]);
/// ```
///
/// Write authority alone does not permit reading:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<WriteOnly>>();
/// file.read();
/// ```
///
/// Narrowing removes replacement authority:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<ReadWrite>>();
/// file.read_append().replace_text("not permitted");
/// ```
///
/// Permissions cannot be widened:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (_, file) = Program::builder::<File<ReadOnly>>();
/// let _: FileCapability<ReadAppend> = file;
/// ```
pub struct FileCapability<P: FilePermissions> {
    pub(crate) token: CapabilityToken,
    marker: PhantomData<fn() -> P>,
}
impl<P: FilePermissions> Clone for FileCapability<P> {
    fn clone(&self) -> Self {
        Self::new(self.token.clone())
    }
}
impl<P: FilePermissions> fmt::Debug for FileCapability<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileCapability")
            .field("permissions", &std::any::type_name::<P>())
            .field("resource", &self.token.id)
            .finish()
    }
}
impl<P: FilePermissions> FileCapability<P> {
    pub(crate) fn new(token: CapabilityToken) -> Self {
        Self {
            token,
            marker: PhantomData,
        }
    }
    fn operation<D: EffectDescriptor>(&self, descriptor: D) -> FileEffect<D> {
        FileEffect {
            capability: EffectCapability {
                token: self.token.clone(),
                marker: PhantomData,
            },
            descriptor,
        }
    }
}
impl<P: CanRead> FileCapability<P> {
    /// Describes reading this file, returning Bytes or FsError when executed.
    pub fn read(&self) -> FileEffect<FileRead> {
        self.operation(FileRead { _private: () })
    }
    /// Shares read-only authority over this same resource.
    pub fn read_only(&self) -> FileCapability<ReadOnly> {
        FileCapability::new(self.token.clone())
    }
}
impl<P: CanAppend> FileCapability<P> {
    /// Describes appending bytes to this file, creating it and its parents if needed.
    pub fn append_bytes(&self, bytes: impl Into<Bytes>) -> FileEffect<FileAppend> {
        self.operation(FileAppend {
            bytes: bytes.into(),
        })
    }
    /// Describes appending UTF-8 text without adding a newline.
    pub fn append_text(&self, text: impl Into<String>) -> FileEffect<FileAppend> {
        self.append_bytes(text.into())
    }
    /// Describes appending text followed by one newline.
    pub fn append_line(&self, text: impl Into<String>) -> FileEffect<FileAppend> {
        let mut text = text.into();
        text.push('\n');
        self.append_text(text)
    }
    /// Shares append-only authority over this same resource.
    pub fn append_only(&self) -> FileCapability<AppendOnly> {
        FileCapability::new(self.token.clone())
    }
}
impl<P: CanRead + CanAppend> FileCapability<P> {
    /// Shares read and append authority without granting replacement.
    pub fn read_append(&self) -> FileCapability<ReadAppend> {
        FileCapability::new(self.token.clone())
    }
}
impl<P: CanWrite> FileCapability<P> {
    /// Describes atomically replacing this file's whole contents.
    ///
    /// Uses [`ReplaceFile`]'s temporary-sibling write, file sync, and rename.
    /// Missing parents are created; the directory entry itself is not synced.
    pub fn replace_bytes(&self, bytes: impl Into<Bytes>) -> FileEffect<FileReplace> {
        self.operation(FileReplace {
            bytes: bytes.into(),
        })
    }
    /// Describes replacing the contents with exactly this text.
    pub fn replace_text(&self, text: impl Into<String>) -> FileEffect<FileReplace> {
        self.replace_bytes(text.into())
    }
    /// Shares write and append authority without granting read access.
    pub fn write_only(&self) -> FileCapability<WriteOnly> {
        FileCapability::new(self.token.clone())
    }
}

/// A typed file operation coupled to the capability that authorized it.
///
/// Convert it to a Command; constructing or inspecting it performs no I/O.
#[must_use = "convert the file operation into a Command"]
pub struct FileEffect<D: EffectDescriptor> {
    capability: EffectCapability<D>,
    descriptor: D,
}
impl<D: EffectDescriptor> FileEffect<D> {
    /// Issues the operation, converting its typed outcome with Message::from.
    pub fn into_command<Message>(self) -> Command<Message>
    where
        Message: From<EffectOutcome<D::Output, D::Error>> + Send + 'static,
    {
        Command::effect(&self.capability, self.descriptor)
    }
    /// Issues the operation with a pure outcome-to-Message mapper.
    pub fn into_command_with<Message, Map>(self, map: Map) -> Command<Message>
    where
        Message: Send + 'static,
        Map: FnOnce(EffectOutcome<D::Output, D::Error>) -> Message + Send + 'static,
    {
        Command::effect_with(&self.capability, self.descriptor, map)
    }
    /// Issues the operation without an application outcome Message.
    pub fn discard_outcome<Message: Send + 'static>(self) -> Command<Message> {
        Command::effect_discarding_outcome(&self.capability, self.descriptor)
    }
}

/// A read of the file selected by its capability, with no target path argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRead {
    _private: (),
}
impl EffectDescriptor for FileRead {
    type Output = Bytes;
    type Error = FsError;
}
/// An append to the file selected by its capability, with no target path argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAppend {
    bytes: Bytes,
}
impl FileAppend {
    /// Returns the bytes that will be appended.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Returns the appended contents as text when they are valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}
impl EffectDescriptor for FileAppend {
    type Output = ();
    type Error = FsError;
}

/// A whole-file replacement at the target selected by its capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReplace {
    bytes: Bytes,
}
impl FileReplace {
    /// Returns the replacement contents.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Returns the replacement contents as text when they are valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}
impl EffectDescriptor for FileReplace {
    type Output = ();
    type Error = FsError;
}

/// Binds all permitted operations of one File capability to one host pathname.
///
/// Uses the existing filesystem mechanisms: reads fail if missing; appends and
/// replacements create the file and its parents. Operations are finite and do not keep an
/// open handle. Await outcomes when one operation must precede another.
#[derive(Clone, Debug)]
pub struct FileDriver {
    fs: FsDriver,
    file: PathBuf,
}
impl FileDriver {
    /// Selects a target without touching the filesystem.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match path.file_name() {
            Some(name) => Self {
                fs: FsDriver::new(
                    path.parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .unwrap_or(Path::new(".")),
                ),
                file: name.into(),
            },
            None => Self {
                fs: FsDriver::new("."),
                file: path,
            },
        }
    }
}
impl EffectDriver<FileRead> for FileDriver {
    fn execute(&self, _: FileRead) -> BoxFuture<Result<Bytes, FsError>> {
        self.fs.execute(ReadFile::new(&self.file))
    }
}
impl EffectDriver<FileAppend> for FileDriver {
    fn execute(&self, operation: FileAppend) -> BoxFuture<Result<(), FsError>> {
        self.fs
            .execute(AppendToFile::bytes(&self.file, operation.bytes))
    }
}
impl EffectDriver<FileReplace> for FileDriver {
    fn execute(&self, operation: FileReplace) -> BoxFuture<Result<(), FsError>> {
        self.fs
            .execute(ReplaceFile::bytes(&self.file, operation.bytes))
    }
}

impl ControlledRuntime {
    /// Takes the next read requested through this exact file capability.
    ///
    /// Clones and narrowed views select the same resource. A foreign selector
    /// fails without taking any pending operation. Completion remains typed:
    ///
    /// ```compile_fail
    /// use samara::prelude::*;
    /// let (builder, file) = Program::builder::<File<ReadOnly>>();
    /// let mut runtime = ControlledRuntime::builder(builder.build().unwrap()).control_all().build().unwrap();
    /// let read = runtime.next_file_read(&file).unwrap();
    /// runtime.complete(read, EffectOutcome::Succeeded(()));
    /// ```
    pub fn next_file_read<P: CanRead>(
        &mut self,
        file: &FileCapability<P>,
    ) -> Result<PendingEffect<FileRead>, RuntimeError> {
        self.core.next_effect_for(Some(&file.token))
    }
    /// Takes the next append requested through this exact file capability.
    pub fn next_file_append<P: CanAppend>(
        &mut self,
        file: &FileCapability<P>,
    ) -> Result<PendingEffect<FileAppend>, RuntimeError> {
        self.core.next_effect_for(Some(&file.token))
    }
    /// Takes the next whole-file replacement requested through this exact capability.
    pub fn next_file_replace<P: CanWrite>(
        &mut self,
        file: &FileCapability<P>,
    ) -> Result<PendingEffect<FileReplace>, RuntimeError> {
        self.core.next_effect_for(Some(&file.token))
    }
}
