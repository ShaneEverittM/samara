//! A fixed file resource with compile-time read, append, and write permissions.
#[cfg(test)]
mod tests;
use crate::{
    AppendToFile, BoxFuture, CapabilityToken, ControlledRuntime, Effect, EffectDescriptor,
    EffectDriver, FsDriver, FsError, PendingEffect, ReadFile, ReplaceFile, RuntimeError,
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

/// Authority to issue permitted operations against one logical file.
///
/// Declare one as a field of a Component's [`Component::Io`](crate::Component::Io).
/// Each operation chooses data, never another target path. The host binds the
/// path once by supplying a [`FileDriver`] for this capability.
///
/// ```
/// use samara::prelude::*;
/// struct Scribe;
/// impl Component for Scribe {
///     type Connections = (); type Configuration = (); type Io = File<ReadAppend>;
///     type Model = (); type Message = ();
///     fn init(cx: &Context<Self>) -> Init<(), ()> {
///         Init::new(()).with_command(File::append_line(&cx.io, "hello").discard_outcome())
///     }
///     fn update(_: &Context<Self>, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let mut program = Program::builder();
/// let scribe = program.add_component::<Scribe>(());
/// let runtime = Runtime::builder(program.build()?)
///     .supply(&scribe.io, FileDriver::new("data/journal.log"))
///     .build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A file takes one Driver, not separate read and append Drivers:
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn two(setup: samara::RuntimeSetup, file: &File<ReadAppend>) {
///     setup.supply(file, (FileDriver::new("a"), FileDriver::new("b")));
/// }
/// ```
///
/// The permission type decides which operations exist:
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn not_permitted(file: &File<ReadOnly>) { File::append_line(file, "no"); }
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn not_permitted(file: &File<AppendOnly>) { File::read(file); }
/// ```
///
/// Append authority does not permit replacement, even when reading is allowed:
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn not_permitted(file: &File<AppendOnly>) { File::replace_text(file, "no"); }
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn not_permitted(file: &File<ReadAppend>) { File::replace_text(file, "no"); }
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn not_permitted(file: &File<ReadOnly>) { File::replace_bytes(file, vec![1, 2, 3]); }
/// ```
///
/// Write authority alone does not permit reading:
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn not_permitted(file: &File<WriteOnly>) { File::read(file); }
/// ```
///
/// Permissions cannot be widened:
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn widen(file: File<ReadOnly>) -> File<ReadAppend> { file }
/// ```
pub struct File<P: FilePermissions> {
    pub(crate) token: CapabilityToken,
    marker: PhantomData<fn() -> P>,
}
impl<P: FilePermissions> Clone for File<P> {
    fn clone(&self) -> Self {
        Self::new(self.token.clone())
    }
}
impl<P: FilePermissions> fmt::Debug for File<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("permissions", &std::any::type_name::<P>())
            .field("resource", &self.token.id)
            .finish()
    }
}
impl<P: FilePermissions> File<P> {
    pub(crate) fn new(token: CapabilityToken) -> Self {
        Self {
            token,
            marker: PhantomData,
        }
    }
    fn operation<D: EffectDescriptor>(&self, descriptor: D) -> FileEffect<D> {
        FileEffect::new(
            Effect {
                token: self.token.clone(),
                marker: PhantomData,
            },
            descriptor,
        )
    }
}
impl<P: CanRead> File<P> {
    /// Describes reading the file, returning Bytes or FsError when executed.
    pub fn read(file: &Self) -> FileEffect<FileRead> {
        file.operation(FileRead { _private: () })
    }
}
impl<P: CanAppend> File<P> {
    /// Describes appending bytes, creating the file and its parents if needed.
    pub fn append_bytes(file: &Self, bytes: impl Into<Bytes>) -> FileEffect<FileAppend> {
        file.operation(FileAppend {
            bytes: bytes.into(),
        })
    }
    /// Describes appending UTF-8 text without adding a newline.
    pub fn append_text(file: &Self, text: impl Into<String>) -> FileEffect<FileAppend> {
        Self::append_bytes(file, text.into())
    }
    /// Describes appending text followed by one newline.
    pub fn append_line(file: &Self, text: impl Into<String>) -> FileEffect<FileAppend> {
        let mut text = text.into();
        text.push('\n');
        Self::append_text(file, text)
    }
}
impl<P: CanWrite> File<P> {
    /// Describes atomically replacing the file's whole contents.
    ///
    /// Uses [`ReplaceFile`]'s temporary-sibling write, file sync, and rename.
    /// Missing parents are created; the directory entry itself is not synced.
    pub fn replace_bytes(file: &Self, bytes: impl Into<Bytes>) -> FileEffect<FileReplace> {
        file.operation(FileReplace {
            bytes: bytes.into(),
        })
    }
    /// Describes replacing the contents with exactly this text.
    pub fn replace_text(file: &Self, text: impl Into<String>) -> FileEffect<FileReplace> {
        Self::replace_bytes(file, text.into())
    }
}

/// A typed file operation coupled to the capability that authorized it.
///
/// This is the general [`EffectBuilder`](crate::EffectBuilder): finish it with
/// `producing`, `into_command`, `finish_with`, or `discard_outcome`.
pub type FileEffect<D> = crate::EffectBuilder<D>;

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
            Some(name) => {
                Self {
                    fs: FsDriver::new(
                        path.parent()
                            .filter(|parent| !parent.as_os_str().is_empty())
                            .unwrap_or(Path::new(".")),
                    ),
                    file: name.into(),
                }
            }
            None => {
                Self {
                    fs: FsDriver::new("."),
                    file: path,
                }
            }
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
    /// Clones select the same resource. A foreign selector
    /// fails without taking any pending operation. Completion remains typed:
    ///
    /// ```compile_fail
    /// use samara::prelude::*;
    /// let file = <File<ReadOnly>>::detached();
    /// let mut runtime = ControlledRuntime::builder(Program::builder().build().unwrap()).build().unwrap();
    /// let read = runtime.next_file_read(&file).unwrap();
    /// runtime.complete(read, EffectOutcome::Succeeded(()));
    /// ```
    pub fn next_file_read<P: CanRead>(
        &mut self,
        file: &File<P>,
    ) -> Result<PendingEffect<FileRead>, RuntimeError> {
        self.core.next_effect_for(Some(&file.token))
    }
    /// Takes the next append requested through this exact file capability.
    pub fn next_file_append<P: CanAppend>(
        &mut self,
        file: &File<P>,
    ) -> Result<PendingEffect<FileAppend>, RuntimeError> {
        self.core.next_effect_for(Some(&file.token))
    }
    /// Takes the next whole-file replacement requested through this exact capability.
    pub fn next_file_replace<P: CanWrite>(
        &mut self,
        file: &File<P>,
    ) -> Result<PendingEffect<FileReplace>, RuntimeError> {
        self.core.next_effect_for(Some(&file.token))
    }
}
