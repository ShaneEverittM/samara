//! Runtime-owned asynchronous execution on Tokio.
//!
//! One owner drives Component kernels while a supervised `JoinSet` runs Drivers
//! and timers. This topology is private: the observable contract is
//! per-Component serialization, explicit causality, and no order promise for
//! independent Driver completions.

use std::{
    any::{Any, TypeId},
    collections::HashSet,
    convert::Infallible,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, MutexGuard},
};

#[cfg(all(unix, target_vendor = "apple"))]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::{
    os::{fd::AsFd, unix::net::UnixStream},
    sync::{
        Condvar, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use bytes::{Bytes, BytesMut};
use futures_util::FutureExt;
#[cfg(unix)]
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
#[cfg(all(unix, target_vendor = "apple"))]
use nix::sys::select::{FdSet, select};
#[cfg(unix)]
use nix::{errno::Errno, unistd::read};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::{AbortHandle, JoinSet},
};

use crate::controlled_runtime::{
    ErasedCommand, ErasedEffectContinuation, ErasedRuntimeSubscription, ErasedSubscriptionChange,
    erase_effect_continuation,
};
use crate::{
    BoxFuture, CapabilityToken, Command, CommandKind, ComponentId, DriverStopped, EffectDescriptor,
    EffectDriver, EffectOutcome, ErasedRequestMapper, ErasedSourceEvent, HttpError, HttpRequest,
    HttpResponse, Notification, Port, PortId, PrintStderr, PrintStdout, Program, Protocol, ReplyTo,
    Request, RequestOutcome, RequestToken, RuntimeError, Shutdown, ShutdownReport,
    SourceCapability, SourceDescriptor, SourceDriver, SourceEvent, SourcePlan, SourceRequirement,
    SourceSink, StreamDescriptor, StreamSourceDescriptor, SubscriptionId, TcpBytes, TcpError,
    TraceId,
};
#[cfg(unix)]
use crate::{StdinError, StdinLines};

type ErasedValue = Box<dyn Any + Send>;
type ErasedMessageMapper =
    Box<dyn FnOnce(ErasedValue) -> ErasedEffectContinuation + Send + 'static>;
type HostReplyCompletion = Box<dyn FnOnce(RequestOutcome<ErasedValue>) + Send + 'static>;
type ProtocolRequestConversion = Box<dyn FnOnce(Arc<RequestToken>) -> ErasedValue + Send + 'static>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One type-erased terminal Effect Driver binding.
pub(crate) trait ErasedEffectBinding: Send + Sync {
    fn descriptor_type(&self) -> TypeId;
    fn descriptor_type_name(&self) -> &'static str;
    fn execute(&self, descriptor: ErasedValue) -> BoxFuture<ErasedValue>;
}

struct TypedEffectBinding<D, Driver> {
    driver: Driver,
    marker: std::marker::PhantomData<fn() -> D>,
}

impl<D, Driver> ErasedEffectBinding for TypedEffectBinding<D, Driver>
where
    D: EffectDescriptor,
    Driver: EffectDriver<D>,
{
    fn descriptor_type(&self) -> TypeId {
        TypeId::of::<D>()
    }

    fn descriptor_type_name(&self) -> &'static str {
        std::any::type_name::<D>()
    }

    fn execute(&self, descriptor: ErasedValue) -> BoxFuture<ErasedValue> {
        let descriptor = *descriptor
            .downcast::<D>()
            .expect("an Effect binding is selected by descriptor TypeId");
        let future = self.driver.execute(descriptor);
        Box::pin(async move {
            let outcome = match future.await {
                Ok(output) => EffectOutcome::<D::Output, D::Error>::Succeeded(output),
                Err(error) => EffectOutcome::<D::Output, D::Error>::Failed(error),
            };
            Box::new(outcome) as ErasedValue
        })
    }
}

/// Runtime-facing sink shared by the public typed `SourceSink` wrapper.
pub(crate) struct SourceSinkCore {
    scope: Arc<LiveScope>,
    gate: Arc<SourceGate>,
    stamp: SourceStamp,
    terminal_type: TypeId,
}

impl SourceSinkCore {
    pub(crate) fn send(
        &self,
        event: ErasedSourceEvent,
        terminal: bool,
    ) -> Result<(), DriverStopped> {
        self.gate.accept(
            &self.scope,
            LiveEvent::SourceEvent {
                stamp: self.stamp.clone(),
                terminal_type: self.terminal_type,
                event,
            },
            terminal,
        )
    }

    fn implicit_end<D: SourceDescriptor>(&self) -> Result<(), DriverStopped> {
        self.send(ErasedSourceEvent::typed::<D>(SourceEvent::Ended), true)
    }
}

/// One type-erased terminal Source Driver or exact bridge binding.
pub(crate) trait ErasedSourceBinding: Send + Sync {
    fn descriptor_type(&self) -> TypeId;
    fn descriptor_type_name(&self) -> &'static str;
    fn is_type_wide(&self) -> bool;
    fn exact_capability(&self) -> Option<&CapabilityToken>;
    fn matches(&self, plan: &SourcePlan) -> bool;
    fn satisfies(&self, requirement: &SourceRequirement) -> bool;
    fn conflicts(&self, other: &dyn ErasedSourceBinding) -> bool;
    fn run(
        &self,
        descriptor: &dyn Any,
        sink: Arc<SourceSinkCore>,
    ) -> BoxFuture<Result<(), Arc<str>>>;
}

struct TypedSourceBinding<D, Driver> {
    driver: Driver,
    marker: std::marker::PhantomData<fn() -> D>,
}

impl<D, Driver> ErasedSourceBinding for TypedSourceBinding<D, Driver>
where
    D: SourceDescriptor,
    Driver: SourceDriver<D>,
{
    fn descriptor_type(&self) -> TypeId {
        TypeId::of::<D>()
    }

    fn descriptor_type_name(&self) -> &'static str {
        std::any::type_name::<D>()
    }

    fn is_type_wide(&self) -> bool {
        true
    }

    fn exact_capability(&self) -> Option<&CapabilityToken> {
        None
    }

    fn matches(&self, plan: &SourcePlan) -> bool {
        plan.terminal_type_id() == TypeId::of::<D>()
    }

    fn satisfies(&self, requirement: &SourceRequirement) -> bool {
        requirement.terminal_type == TypeId::of::<D>()
    }

    fn conflicts(&self, other: &dyn ErasedSourceBinding) -> bool {
        other.descriptor_type() == TypeId::of::<D>()
    }

    fn run(
        &self,
        descriptor: &dyn Any,
        sink: Arc<SourceSinkCore>,
    ) -> BoxFuture<Result<(), Arc<str>>> {
        let descriptor = descriptor
            .downcast_ref::<D>()
            .expect("a Source binding is selected by descriptor TypeId")
            .clone();
        let typed_sink = SourceSink::runtime(sink.clone());
        let future = self.driver.run(descriptor, typed_sink);
        Box::pin(async move {
            future.await;
            let _ = sink.implicit_end::<D>();
            Ok(())
        })
    }
}

struct MpscBinding<T> {
    capability: CapabilityToken,
    receiver: Arc<Mutex<Option<mpsc::Receiver<T>>>>,
}

impl<T: Send + 'static> ErasedSourceBinding for MpscBinding<T> {
    fn descriptor_type(&self) -> TypeId {
        TypeId::of::<StreamDescriptor<T>>()
    }

    fn descriptor_type_name(&self) -> &'static str {
        std::any::type_name::<StreamDescriptor<T>>()
    }

    fn is_type_wide(&self) -> bool {
        false
    }

    fn exact_capability(&self) -> Option<&CapabilityToken> {
        Some(&self.capability)
    }

    fn matches(&self, plan: &SourcePlan) -> bool {
        plan.terminal_type_id() == TypeId::of::<StreamDescriptor<T>>()
            && self.capability.same_as(plan.capability())
    }

    fn satisfies(&self, requirement: &SourceRequirement) -> bool {
        requirement.terminal_type == TypeId::of::<StreamDescriptor<T>>()
            && self.capability.same_as(&requirement.token)
    }

    fn conflicts(&self, other: &dyn ErasedSourceBinding) -> bool {
        if other.descriptor_type() != TypeId::of::<StreamDescriptor<T>>() {
            return false;
        }
        other.is_type_wide()
            || other
                .exact_capability()
                .is_some_and(|capability| capability.same_as(&self.capability))
    }

    fn run(
        &self,
        descriptor: &dyn Any,
        sink: Arc<SourceSinkCore>,
    ) -> BoxFuture<Result<(), Arc<str>>> {
        let matches = descriptor.is::<StreamDescriptor<T>>();
        let receiver = matches.then(|| lock(&self.receiver).take()).flatten();
        Box::pin(async move {
            if !matches {
                return Err(Arc::from("mpsc binding descriptor mismatch"));
            }
            let mut receiver =
                receiver.ok_or_else(|| Arc::from("one-shot mpsc binding was already consumed"))?;
            while let Some(item) = receiver.recv().await {
                if sink
                    .send(
                        ErasedSourceEvent::typed::<StreamDescriptor<T>>(SourceEvent::Item(item)),
                        false,
                    )
                    .is_err()
                {
                    // Drain, Cancel, replacement, and removal are ordinary
                    // Source cutovers. The bridge releases its receiver rather
                    // than misclassifying `DriverStopped` as a mechanism fault.
                    return Ok(());
                }
            }
            let _ = sink.implicit_end::<StreamDescriptor<T>>();
            Ok(())
        })
    }
}

#[cfg(unix)]
struct StdinBinding {
    capability: CapabilityToken,
    active: Arc<AtomicBool>,
}

#[cfg(unix)]
fn process_stdin_lease() -> Arc<AtomicBool> {
    static ACTIVE: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    ACTIVE
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

#[cfg(unix)]
impl StdinBinding {
    fn acquire<Input>(&self, input: Input) -> Result<StdinReader, Arc<str>>
    where
        Input: AsFd + Send + 'static,
    {
        if self
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Arc::from(
                "process standard input already has an active Source realization",
            ));
        }
        match StdinReader::spawn(input, self.active.clone()) {
            Ok(reader) => Ok(reader),
            Err(error) => {
                self.active.store(false, Ordering::Release);
                Err(error)
            }
        }
    }
}

#[cfg(unix)]
impl ErasedSourceBinding for StdinBinding {
    fn descriptor_type(&self) -> TypeId {
        TypeId::of::<StdinLines>()
    }

    fn descriptor_type_name(&self) -> &'static str {
        std::any::type_name::<StdinLines>()
    }

    fn is_type_wide(&self) -> bool {
        false
    }

    fn exact_capability(&self) -> Option<&CapabilityToken> {
        Some(&self.capability)
    }

    fn matches(&self, plan: &SourcePlan) -> bool {
        plan.terminal_type_id() == TypeId::of::<StdinLines>()
            && self.capability.same_as(plan.capability())
    }

    fn satisfies(&self, requirement: &SourceRequirement) -> bool {
        requirement.terminal_type == TypeId::of::<StdinLines>()
            && self.capability.same_as(&requirement.token)
    }

    fn conflicts(&self, other: &dyn ErasedSourceBinding) -> bool {
        other.descriptor_type() == TypeId::of::<StdinLines>()
    }

    fn run(
        &self,
        descriptor: &dyn Any,
        sink: Arc<SourceSinkCore>,
    ) -> BoxFuture<Result<(), Arc<str>>> {
        if !descriptor.is::<StdinLines>() {
            return Box::pin(async { Err(Arc::from("stdin binding descriptor mismatch")) });
        }
        let reader = match self.acquire(std::io::stdin()) {
            Ok(reader) => reader,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let control = reader.control.clone();
        let installed = sink.gate.install_stop_hook(move || control.stop_and_join());
        match installed {
            Ok(true) => {}
            Ok(false) => return Box::pin(async { Ok(()) }),
            Err(error) => return Box::pin(async move { Err(error) }),
        }
        Box::pin(run_stdin_reader(reader, sink))
    }
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum StdinReaderEvent {
    Line(String),
    Failed(StdinError),
    Ended,
    Cancelled,
}

#[cfg(unix)]
enum StdinReadiness {
    Input,
    Cancelled,
}

#[cfg(unix)]
enum StdinReadinessError {
    Input(StdinError),
    Mechanism(Arc<str>),
}

#[cfg(unix)]
#[derive(Default)]
struct StdinLineDecoder {
    pending: Vec<u8>,
}

#[cfg(unix)]
impl StdinLineDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<StdinReaderEvent> {
        let previously_buffered = self.pending.len();
        self.pending.extend_from_slice(bytes);
        let Some(last_newline) = self.pending[previously_buffered..]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| previously_buffered + index)
        else {
            return Vec::new();
        };
        let complete = self.pending.drain(..=last_newline).collect::<Vec<_>>();
        let mut events = Vec::new();
        for encoded in complete.split_inclusive(|byte| *byte == b'\n') {
            let without_lf = &encoded[..encoded.len() - 1];
            let line = without_lf.strip_suffix(b"\r").unwrap_or(without_lf);
            match String::from_utf8(line.to_vec()) {
                Ok(line) => events.push(StdinReaderEvent::Line(line)),
                Err(error) => {
                    self.pending.clear();
                    events.push(StdinReaderEvent::Failed(StdinError::invalid_utf8(error)));
                    break;
                }
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<StdinReaderEvent> {
        if self.pending.is_empty() {
            return vec![StdinReaderEvent::Ended];
        }
        let encoded = std::mem::take(&mut self.pending);
        match String::from_utf8(encoded) {
            Ok(line) => vec![StdinReaderEvent::Line(line), StdinReaderEvent::Ended],
            Err(error) => vec![StdinReaderEvent::Failed(StdinError::invalid_utf8(error))],
        }
    }
}

#[cfg(unix)]
struct StdinReader {
    events: mpsc::UnboundedReceiver<StdinReaderEvent>,
    control: Arc<StdinReaderControl>,
}

#[cfg(unix)]
struct StdinReaderControl {
    state: Mutex<StdinReaderControlState>,
    stopped: Condvar,
}

#[cfg(unix)]
enum StdinReaderControlState {
    Running {
        cancel: UnixStream,
        thread: JoinHandle<Result<(), Arc<str>>>,
        active: Arc<AtomicBool>,
    },
    Stopping,
    Stopped(Result<(), Arc<str>>),
}

#[cfg(unix)]
impl StdinReaderControl {
    fn stop_and_join(&self) -> Result<(), Arc<str>> {
        let (cancel, thread, active) = {
            let mut state = lock(&self.state);
            loop {
                match &*state {
                    StdinReaderControlState::Running { .. } => {
                        let StdinReaderControlState::Running {
                            cancel,
                            thread,
                            active,
                        } = std::mem::replace(&mut *state, StdinReaderControlState::Stopping)
                        else {
                            unreachable!("the reader state was just matched as running")
                        };
                        break (cancel, thread, active);
                    }
                    StdinReaderControlState::Stopping => {
                        state = self
                            .stopped
                            .wait(state)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    StdinReaderControlState::Stopped(result) => return result.clone(),
                }
            }
        };

        let _ = cancel.shutdown(std::net::Shutdown::Both);
        let result = thread
            .join()
            .map_err(|_| Arc::from("standard-input reader thread panicked"))
            .and_then(std::convert::identity);
        active.store(false, Ordering::Release);

        let mut state = lock(&self.state);
        *state = StdinReaderControlState::Stopped(result.clone());
        self.stopped.notify_all();
        result
    }
}

#[cfg(unix)]
impl StdinReader {
    fn spawn<Input>(input: Input, active: Arc<AtomicBool>) -> Result<Self, Arc<str>>
    where
        Input: AsFd + Send + 'static,
    {
        let (cancel_read, cancel_write) = UnixStream::pair().map_err(|error| {
            Arc::from(format!(
                "could not create stdin cancellation channel: {error}"
            ))
        })?;
        let (sender, events) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("samara-stdin-lines".to_owned())
            .spawn(move || drive_stdin_reader(input, cancel_read, sender))
            .map_err(|error| Arc::from(format!("could not start stdin reader thread: {error}")))?;
        Ok(Self {
            events,
            control: Arc::new(StdinReaderControl {
                state: Mutex::new(StdinReaderControlState::Running {
                    cancel: cancel_write,
                    thread,
                    active,
                }),
                stopped: Condvar::new(),
            }),
        })
    }

    fn stop_and_join(&self) -> Result<(), Arc<str>> {
        self.control.stop_and_join()
    }
}

#[cfg(unix)]
impl Drop for StdinReader {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

#[cfg(unix)]
fn drive_stdin_reader<Input>(
    input: Input,
    cancel: UnixStream,
    sender: mpsc::UnboundedSender<StdinReaderEvent>,
) -> Result<(), Arc<str>>
where
    Input: AsFd,
{
    let mut decoder = StdinLineDecoder::default();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match wait_for_stdin(&input, &cancel) {
            Ok(StdinReadiness::Cancelled) => {
                let _ = sender.send(StdinReaderEvent::Cancelled);
                return Ok(());
            }
            Ok(StdinReadiness::Input) => {}
            Err(StdinReadinessError::Input(error)) => {
                let _ = sender.send(StdinReaderEvent::Failed(error));
                return Ok(());
            }
            Err(StdinReadinessError::Mechanism(error)) => {
                return Err(error);
            }
        }
        match read(&input, &mut buffer) {
            Err(Errno::EINTR | Errno::EAGAIN) => continue,
            Err(error) => {
                let _ = sender.send(StdinReaderEvent::Failed(StdinError::read(error)));
                return Ok(());
            }
            Ok(0) => {
                send_stdin_events(&sender, decoder.finish());
                return Ok(());
            }
            Ok(length) => {
                if !send_stdin_events(&sender, decoder.push(&buffer[..length])) {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(all(unix, target_vendor = "apple"))]
fn wait_for_stdin<Input>(
    input: &Input,
    cancel: &UnixStream,
) -> Result<StdinReadiness, StdinReadinessError>
where
    Input: AsFd,
{
    loop {
        let input_fd = input.as_fd();
        let cancel_fd = cancel.as_fd();
        if [input_fd.as_raw_fd(), cancel_fd.as_raw_fd()]
            .into_iter()
            .any(|fd| fd < 0 || fd as usize >= nix::sys::select::FD_SETSIZE)
        {
            return Err(StdinReadinessError::Mechanism(Arc::from(
                "standard-input readiness file descriptor exceeds the Unix select limit",
            )));
        }
        let mut readable = FdSet::new();
        readable.insert(input_fd);
        readable.insert(cancel_fd);
        match select(None, Some(&mut readable), None, None, None) {
            Err(Errno::EINTR) => continue,
            Err(error) => {
                return Err(StdinReadinessError::Mechanism(Arc::from(format!(
                    "standard-input readiness mechanism failed: {error}"
                ))));
            }
            Ok(_) if readable.contains(cancel_fd) => {
                // Cutover wins whenever both descriptors are observed ready.
                return Ok(StdinReadiness::Cancelled);
            }
            Ok(_) if readable.contains(input_fd) => return Ok(StdinReadiness::Input),
            Ok(_) => continue,
        }
    }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn wait_for_stdin<Input>(
    input: &Input,
    cancel: &UnixStream,
) -> Result<StdinReadiness, StdinReadinessError>
where
    Input: AsFd,
{
    wait_for_stdin_with_poll(input, cancel)
}

#[cfg(unix)]
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
fn wait_for_stdin_with_poll<Input>(
    input: &Input,
    cancel: &UnixStream,
) -> Result<StdinReadiness, StdinReadinessError>
where
    Input: AsFd,
{
    const CANCEL_READY: PollFlags =
        PollFlags::from_bits_retain(PollFlags::POLLIN.bits() | PollFlags::POLLHUP.bits());
    const CANCEL_ERROR: PollFlags =
        PollFlags::from_bits_retain(PollFlags::POLLERR.bits() | PollFlags::POLLNVAL.bits());
    const INPUT_READY: PollFlags = PollFlags::from_bits_retain(
        PollFlags::POLLIN.bits() | PollFlags::POLLHUP.bits() | PollFlags::POLLERR.bits(),
    );
    loop {
        let mut descriptors = [
            PollFd::new(input.as_fd(), PollFlags::POLLIN),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut descriptors, PollTimeout::NONE) {
            Err(Errno::EINTR) => continue,
            Err(error) => {
                return Err(StdinReadinessError::Mechanism(Arc::from(format!(
                    "standard-input readiness mechanism failed: {error}"
                ))));
            }
            Ok(_) => {}
        }
        let cancel_events = descriptors[1].revents().unwrap_or_else(PollFlags::empty);
        if cancel_events.intersects(CANCEL_ERROR) {
            return Err(StdinReadinessError::Mechanism(Arc::from(format!(
                "standard-input cancellation channel failed: {cancel_events:?}"
            ))));
        }
        if cancel_events.intersects(CANCEL_READY) {
            return Ok(StdinReadiness::Cancelled);
        }
        if !cancel_events.is_empty() {
            return Err(StdinReadinessError::Mechanism(Arc::from(format!(
                "unexpected stdin cancellation readiness flags: {cancel_events:?}"
            ))));
        }

        let input_events = descriptors[0].revents().unwrap_or_else(PollFlags::empty);
        if input_events.contains(PollFlags::POLLNVAL) {
            return Err(StdinReadinessError::Input(StdinError::new(
                crate::StdinErrorKind::Read,
                "standard-input file descriptor is invalid",
            )));
        }
        if input_events.intersects(INPUT_READY) {
            return Ok(StdinReadiness::Input);
        }
        if !input_events.is_empty() {
            return Err(StdinReadinessError::Input(StdinError::new(
                crate::StdinErrorKind::Read,
                format!("unexpected standard-input readiness flags: {input_events:?}"),
            )));
        }
    }
}

#[cfg(unix)]
fn send_stdin_events(
    sender: &mpsc::UnboundedSender<StdinReaderEvent>,
    events: Vec<StdinReaderEvent>,
) -> bool {
    for event in events {
        let terminal = matches!(event, StdinReaderEvent::Failed(_) | StdinReaderEvent::Ended);
        if sender.send(event).is_err() || terminal {
            return false;
        }
    }
    true
}

#[cfg(unix)]
async fn run_stdin_reader(
    mut reader: StdinReader,
    sink: Arc<SourceSinkCore>,
) -> Result<(), Arc<str>> {
    let outcome = loop {
        let Some(event) = reader.events.recv().await else {
            break Err(Arc::from(
                "standard-input reader stopped without a terminal event",
            ));
        };
        match event {
            StdinReaderEvent::Line(line) => {
                if sink
                    .send(
                        ErasedSourceEvent::typed::<StdinLines>(SourceEvent::Item(line)),
                        false,
                    )
                    .is_err()
                {
                    break Ok(());
                }
            }
            StdinReaderEvent::Failed(error) => {
                let _ = sink.send(
                    ErasedSourceEvent::typed::<StdinLines>(SourceEvent::Failed(error)),
                    true,
                );
                break Ok(());
            }
            StdinReaderEvent::Ended => {
                let _ = sink.implicit_end::<StdinLines>();
                break Ok(());
            }
            StdinReaderEvent::Cancelled => break Ok(()),
        }
    };
    let joined = reader.stop_and_join();
    match joined {
        Ok(()) => outcome,
        Err(error) => Err(error),
    }
}

struct TokioTcpDriver;

struct TokioHttpDriver {
    client: Result<reqwest::Client, HttpError>,
}

impl TokioHttpDriver {
    fn new() -> Self {
        let builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .tcp_keepalive(None::<std::time::Duration>);
        #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
        let builder = builder.tcp_user_timeout(None::<std::time::Duration>);
        let client = builder.build().map_err(HttpError::configuration);
        Self { client }
    }
}

impl EffectDriver<HttpRequest> for TokioHttpDriver {
    fn execute(&self, descriptor: HttpRequest) -> BoxFuture<Result<HttpResponse, HttpError>> {
        let client = match &self.client {
            Ok(client) => client.clone(),
            Err(error) => {
                let error = error.clone();
                return Box::pin(async move { Err(error) });
            }
        };
        let (method, url, mut headers, body) = descriptor.into_parts();
        Box::pin(async move {
            if !headers.contains_key(http::header::ACCEPT) {
                // Make reqwest's no-preference wire default explicit rather
                // than inheriting it as accidental hidden behavior.
                headers.insert(http::header::ACCEPT, http::HeaderValue::from_static("*/*"));
            }
            let request = client
                .request(method, url.as_ref())
                .headers(headers)
                .body(body)
                .build()
                .map_err(HttpError::configuration)?;
            let response = client
                .execute(request)
                .await
                .map_err(HttpError::transport)?;
            let status = response.status();
            let version = response.version();
            let headers = response.headers().clone();
            let body = response.bytes().await.map_err(HttpError::transport)?;
            Ok(HttpResponse::new(status, version, headers, body))
        })
    }
}

struct TokioPrintStdoutDriver {
    output: Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
}

impl TokioPrintStdoutDriver {
    fn new() -> Self {
        Self {
            output: Arc::new(tokio::sync::Mutex::new(tokio::io::stdout())),
        }
    }
}

impl EffectDriver<PrintStdout> for TokioPrintStdoutDriver {
    fn execute(&self, descriptor: PrintStdout) -> BoxFuture<Result<(), Infallible>> {
        let output = self.output.clone();
        let text = descriptor.into_text();
        Box::pin(print_best_effort(output, Bytes::from(text)))
    }
}

struct TokioPrintStderrDriver {
    output: Arc<tokio::sync::Mutex<tokio::io::Stderr>>,
}

impl TokioPrintStderrDriver {
    fn new() -> Self {
        Self {
            output: Arc::new(tokio::sync::Mutex::new(tokio::io::stderr())),
        }
    }
}

impl EffectDriver<PrintStderr> for TokioPrintStderrDriver {
    fn execute(&self, descriptor: PrintStderr) -> BoxFuture<Result<(), Infallible>> {
        let output = self.output.clone();
        let text = descriptor.into_text();
        Box::pin(print_best_effort(output, Bytes::from(text)))
    }
}

async fn print_best_effort<Writer>(
    output: Arc<tokio::sync::Mutex<Writer>>,
    bytes: Bytes,
) -> Result<(), Infallible>
where
    Writer: AsyncWrite + Unpin,
{
    let _ = write_and_flush(output, bytes).await;
    Ok(())
}

async fn write_and_flush<Writer>(
    output: Arc<tokio::sync::Mutex<Writer>>,
    bytes: Bytes,
) -> Result<(), std::io::Error>
where
    Writer: AsyncWrite + Unpin,
{
    let mut output = output.lock().await;
    output.write_all(&bytes).await?;
    output.flush().await
}

async fn drive_tcp_reader<Reader>(reader: &mut Reader, sink: SourceSink<TcpBytes>)
where
    Reader: AsyncRead + Unpin,
{
    let mut buffer = BytesMut::with_capacity(8 * 1024);
    loop {
        match reader.read_buf(&mut buffer).await {
            Ok(0) => {
                let _ = sink.end().await;
                return;
            }
            Ok(_) => {
                let bytes = buffer.split().freeze();
                if sink.emit(bytes).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sink.fail(TcpError::read(error)).await;
                return;
            }
        }
    }
}

impl SourceDriver<TcpBytes> for TokioTcpDriver {
    fn run(&self, descriptor: TcpBytes, sink: SourceSink<TcpBytes>) -> BoxFuture<()> {
        Box::pin(async move {
            let mut stream = match TcpStream::connect(descriptor.endpoint()).await {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = sink.fail(TcpError::connect(error)).await;
                    return;
                }
            };
            drive_tcp_reader(&mut stream, sink).await;
        })
    }
}

/// Erased Driver registry retained by a live assembly.
pub(crate) struct LiveBindings {
    effects: Vec<Arc<dyn ErasedEffectBinding>>,
    sources: Vec<Arc<dyn ErasedSourceBinding>>,
    #[cfg(unix)]
    stdin_requests: u8,
}

impl LiveBindings {
    pub(crate) fn new() -> Self {
        Self {
            effects: Vec::new(),
            sources: Vec::new(),
            #[cfg(unix)]
            stdin_requests: 0,
        }
    }

    pub(crate) fn bind_effect<D, Driver>(&mut self, driver: Driver)
    where
        D: EffectDescriptor,
        Driver: EffectDriver<D>,
    {
        self.effects.push(Arc::new(TypedEffectBinding::<D, Driver> {
            driver,
            marker: std::marker::PhantomData,
        }));
    }

    pub(crate) fn bind_source<D, Driver>(&mut self, driver: Driver)
    where
        D: SourceDescriptor,
        Driver: SourceDriver<D>,
    {
        self.sources.push(Arc::new(TypedSourceBinding::<D, Driver> {
            driver,
            marker: std::marker::PhantomData,
        }));
    }

    pub(crate) fn bind_mpsc<S>(
        &mut self,
        capability: &SourceCapability<S>,
        receiver: mpsc::Receiver<S::StreamItem>,
    ) where
        S: StreamSourceDescriptor,
    {
        self.sources.push(Arc::new(MpscBinding::<S::StreamItem> {
            capability: capability.token.clone(),
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }));
    }

    #[cfg(unix)]
    pub(crate) fn bind_stdin(&mut self) {
        self.stdin_requests = self.stdin_requests.saturating_add(1);
    }

    #[cfg(unix)]
    fn resolve_stdin(&mut self, program: &Program) -> Result<(), RuntimeError> {
        match self.stdin_requests {
            0 => return Ok(()),
            1 => {}
            _ => return Err(RuntimeError::harness("duplicate live stdin binding")),
        }
        let mut candidates = program
            .source_requirements
            .iter()
            .filter(|requirement| requirement.terminal_type == TypeId::of::<StdinLines>());
        let capability = candidates.next().ok_or_else(|| {
            RuntimeError::harness("bind_stdin() has no declared stdin capability")
        })?;
        if candidates.next().is_some() {
            return Err(RuntimeError::harness(
                "bind_stdin() has multiple declared stdin capabilities; expected exactly one",
            ));
        }
        self.sources.push(Arc::new(StdinBinding {
            capability: capability.token.clone(),
            active: process_stdin_lease(),
        }));
        self.stdin_requests = 0;
        Ok(())
    }

    pub(crate) fn bind_tcp(&mut self) {
        self.bind_source::<TcpBytes, _>(TokioTcpDriver);
    }

    pub(crate) fn bind_http(&mut self) {
        self.bind_effect::<HttpRequest, _>(TokioHttpDriver::new());
    }

    pub(crate) fn bind_stdio(&mut self) {
        self.bind_effect::<PrintStdout, _>(TokioPrintStdoutDriver::new());
        self.bind_effect::<PrintStderr, _>(TokioPrintStderrDriver::new());
    }

    pub(crate) fn validate(&mut self, program: &Program) -> Result<(), RuntimeError> {
        #[cfg(unix)]
        self.resolve_stdin(program)?;

        let mut effect_types = HashSet::new();
        for binding in &self.effects {
            if !effect_types.insert(binding.descriptor_type()) {
                return Err(RuntimeError::harness(format!(
                    "duplicate live Effect binding for {}",
                    binding.descriptor_type_name()
                )));
            }
        }
        for (index, binding) in self.sources.iter().enumerate() {
            if self.sources[index + 1..]
                .iter()
                .any(|other| binding.conflicts(other.as_ref()) || other.conflicts(binding.as_ref()))
            {
                return Err(RuntimeError::harness(format!(
                    "duplicate or ambiguous live Source binding for {}",
                    binding.descriptor_type_name()
                )));
            }

            if let Some(capability) = binding.exact_capability() {
                if !capability.belongs_to(&program.program) {
                    return Err(RuntimeError::harness(format!(
                        "exact live Source binding for {} uses a capability from another Program",
                        binding.descriptor_type_name()
                    )));
                }
                let Some(requirement) = program
                    .source_requirements
                    .iter()
                    .find(|requirement| requirement.token.same_as(capability))
                else {
                    return Err(RuntimeError::harness(format!(
                        "exact live Source binding for {} has no declared Program capability",
                        binding.descriptor_type_name()
                    )));
                };
                if requirement.terminal_type != binding.descriptor_type() {
                    return Err(RuntimeError::harness(format!(
                        "exact live Source binding for {} does not match declared terminal {}",
                        binding.descriptor_type_name(),
                        requirement.terminal_type_name
                    )));
                }
            }
        }

        for requirement in &program.effect_requirements {
            let count = self
                .effects
                .iter()
                .filter(|binding| binding.descriptor_type() == requirement.descriptor_type)
                .count();
            if count != 1 {
                return Err(RuntimeError::harness(format!(
                    "declared Effect capability for {} has {count} live bindings; expected exactly one",
                    requirement.descriptor_type_name
                )));
            }
        }
        for requirement in &program.source_requirements {
            let count = self
                .sources
                .iter()
                .filter(|binding| binding.satisfies(requirement))
                .count();
            if count != 1 {
                return Err(RuntimeError::harness(format!(
                    "declared Source capability for {} (terminal {}) has {count} live bindings; expected exactly one",
                    requirement.descriptor_type_name, requirement.terminal_type_name
                )));
            }
        }
        Ok(())
    }

    fn effect(&self, descriptor_type: TypeId) -> Option<Arc<dyn ErasedEffectBinding>> {
        self.effects
            .iter()
            .find(|binding| binding.descriptor_type() == descriptor_type)
            .cloned()
    }

    fn source(&self, plan: &SourcePlan) -> Result<Arc<dyn ErasedSourceBinding>, &'static str> {
        let matches = self
            .sources
            .iter()
            .filter(|binding| binding.matches(plan))
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [binding] => Ok(binding.clone()),
            [] => Err("missing live terminal Source binding"),
            _ => Err("ambiguous live terminal Source binding"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SourceStamp {
    component: ComponentId,
    subscription: SubscriptionId,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceGateState {
    Active,
    TerminalAccepted,
    Stopped,
}

struct SourceGate {
    inner: Mutex<SourceGateInner>,
}

#[cfg(unix)]
type SourceStopHook = Box<dyn FnOnce() -> Result<(), Arc<str>> + Send + 'static>;

struct SourceGateInner {
    state: SourceGateState,
    #[cfg(unix)]
    stop_hook: Option<SourceStopHook>,
}

impl SourceGate {
    fn new() -> Self {
        Self {
            inner: Mutex::new(SourceGateInner {
                state: SourceGateState::Active,
                #[cfg(unix)]
                stop_hook: None,
            }),
        }
    }

    fn accept(
        &self,
        scope: &LiveScope,
        event: LiveEvent,
        terminal: bool,
    ) -> Result<(), DriverStopped> {
        let mut inner = lock(&self.inner);
        if inner.state != SourceGateState::Active {
            return Err(DriverStopped);
        }
        scope.accept_source(event)?;
        if terminal {
            inner.state = SourceGateState::TerminalAccepted;
        }
        Ok(())
    }

    fn stop(&self) -> Result<(), Arc<str>> {
        #[cfg(unix)]
        let hook = {
            let mut inner = lock(&self.inner);
            inner.state = SourceGateState::Stopped;
            inner.stop_hook.take()
        };
        #[cfg(not(unix))]
        {
            lock(&self.inner).state = SourceGateState::Stopped;
            Ok(())
        }
        #[cfg(unix)]
        {
            hook.map_or(Ok(()), |hook| hook())
        }
    }

    #[cfg(unix)]
    fn install_stop_hook(
        &self,
        hook: impl FnOnce() -> Result<(), Arc<str>> + Send + 'static,
    ) -> Result<bool, Arc<str>> {
        let mut hook = Some(Box::new(hook) as SourceStopHook);
        let run_now = {
            let mut inner = lock(&self.inner);
            if inner.state == SourceGateState::Stopped {
                true
            } else {
                assert!(
                    inner.stop_hook.is_none(),
                    "a Source realization may install at most one stop hook"
                );
                inner.stop_hook = hook.take();
                false
            }
        };
        if run_now {
            hook.expect("a stopped gate retains the uninstalled hook")()?;
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

enum ScopePhase {
    Running,
    Draining,
    Cancelling,
    Faulted(RuntimeError),
    Closed,
}

struct ScopeState {
    phase: ScopePhase,
    sender: mpsc::UnboundedSender<LiveEvent>,
}

/// Shared admission and completion boundary used by handles and Driver tasks.
pub(crate) struct LiveScope {
    state: Mutex<ScopeState>,
}

impl LiveScope {
    pub(crate) fn new(sender: mpsc::UnboundedSender<LiveEvent>) -> Self {
        Self {
            state: Mutex::new(ScopeState {
                phase: ScopePhase::Running,
                sender,
            }),
        }
    }

    fn is_running(&self) -> bool {
        matches!(lock(&self.state).phase, ScopePhase::Running)
    }

    fn is_cancelling(&self) -> bool {
        matches!(lock(&self.state).phase, ScopePhase::Cancelling)
    }

    fn is_draining(&self) -> bool {
        matches!(lock(&self.state).phase, ScopePhase::Draining)
    }

    pub(crate) fn accept_ingress(&self, event: LiveEvent) -> Result<(), RuntimeError> {
        let state = lock(&self.state);
        match &state.phase {
            ScopePhase::Running => state
                .sender
                .send(event)
                .map_err(|_| RuntimeError::harness("live runtime task is no longer running")),
            ScopePhase::Faulted(error) => Err(error.clone()),
            ScopePhase::Draining | ScopePhase::Cancelling | ScopePhase::Closed => {
                Err(RuntimeError::harness("live runtime ingress is closed"))
            }
        }
    }

    pub(crate) fn request_ended_error(&self) -> RuntimeError {
        match &lock(&self.state).phase {
            ScopePhase::Faulted(error) => error.clone(),
            ScopePhase::Running
            | ScopePhase::Draining
            | ScopePhase::Cancelling
            | ScopePhase::Closed => RuntimeError::harness("live runtime ended before a reply"),
        }
    }

    fn accept_source(&self, event: LiveEvent) -> Result<(), DriverStopped> {
        let state = lock(&self.state);
        if !matches!(state.phase, ScopePhase::Running) {
            return Err(DriverStopped);
        }
        state.sender.send(event).map_err(|_| DriverStopped)
    }

    fn accept_finite(&self, event: LiveEvent) -> bool {
        let state = lock(&self.state);
        if !matches!(state.phase, ScopePhase::Running | ScopePhase::Draining) {
            return false;
        }
        state.sender.send(event).is_ok()
    }

    pub(crate) fn begin_shutdown(&self, mode: Shutdown) -> Result<(), RuntimeError> {
        let mut state = lock(&self.state);
        match (&state.phase, mode) {
            (ScopePhase::Running, _) | (ScopePhase::Draining, Shutdown::Cancel) => {
                state.phase = match mode {
                    Shutdown::Drain => ScopePhase::Draining,
                    Shutdown::Cancel => ScopePhase::Cancelling,
                };
                state
                    .sender
                    .send(LiveEvent::Shutdown(mode))
                    .map_err(|_| RuntimeError::harness("live runtime task ended before shutdown"))
            }
            (ScopePhase::Faulted(error), _) => Err(error.clone()),
            // Requests only strengthen shutdown. Repetition neither queues
            // another marker nor reopens admission after cancellation/closure.
            (ScopePhase::Draining | ScopePhase::Cancelling | ScopePhase::Closed, _) => Ok(()),
        }
    }

    fn fault(&self, error: RuntimeError) {
        let mut state = lock(&self.state);
        if !matches!(state.phase, ScopePhase::Faulted(_) | ScopePhase::Closed) {
            state.phase = ScopePhase::Faulted(error);
        }
    }

    fn close(&self) {
        lock(&self.state).phase = ScopePhase::Closed;
    }
}

pub(crate) struct LiveIngress {
    pub(crate) target: ComponentId,
    pub(crate) target_message_type: TypeId,
    pub(crate) message_type_name: &'static str,
    pub(crate) message: ErasedValue,
}

pub(crate) enum LivePortIngress {
    Notification(Box<dyn ErasedLivePortNotification>),
    Request(Box<dyn ErasedLivePortRequest>),
}

pub(crate) trait ErasedLivePortNotification: Send {
    fn port(&self) -> &PortId;
    fn port_program(&self) -> &Arc<()>;
    fn protocol_type(&self) -> TypeId;
    fn into_message(self: Box<Self>) -> ErasedValue;
}

pub(crate) trait ErasedLivePortRequest: Send {
    fn port(&self) -> &PortId;
    fn port_program(&self) -> &Arc<()>;
    fn protocol_type(&self) -> TypeId;
    fn reply_type(&self) -> TypeId;
    fn deadline(&self) -> Option<tokio::time::Instant>;
    fn into_conversion(self: Box<Self>) -> (ProtocolRequestConversion, HostReplyCompletion);
}

struct TypedLivePortNotification<P, N>
where
    P: Protocol,
    N: Notification<P>,
{
    port: Port<P>,
    notification: N,
}

impl<P, N> ErasedLivePortNotification for TypedLivePortNotification<P, N>
where
    P: Protocol,
    N: Notification<P>,
{
    fn port(&self) -> &PortId {
        self.port.id()
    }

    fn port_program(&self) -> &Arc<()> {
        &self.port.program
    }

    fn protocol_type(&self) -> TypeId {
        TypeId::of::<P>()
    }

    fn into_message(self: Box<Self>) -> ErasedValue {
        Box::new(self.notification.into_message())
    }
}

struct TypedLivePortRequest<P, R>
where
    P: Protocol,
    R: Request<P>,
{
    port: Port<P>,
    request: R,
    deadline: Option<tokio::time::Instant>,
    completion: oneshot::Sender<RequestOutcome<R::Reply>>,
}

impl<P, R> ErasedLivePortRequest for TypedLivePortRequest<P, R>
where
    P: Protocol,
    R: Request<P>,
{
    fn port(&self) -> &PortId {
        self.port.id()
    }

    fn port_program(&self) -> &Arc<()> {
        &self.port.program
    }

    fn protocol_type(&self) -> TypeId {
        TypeId::of::<P>()
    }

    fn reply_type(&self) -> TypeId {
        TypeId::of::<R::Reply>()
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline
    }

    fn into_conversion(self: Box<Self>) -> (ProtocolRequestConversion, HostReplyCompletion) {
        let Self {
            request,
            completion,
            ..
        } = *self;
        let conversion: ProtocolRequestConversion =
            Box::new(move |token| Box::new(request.into_message(ReplyTo::runtime(token))));
        let completion: HostReplyCompletion = Box::new(move |outcome| {
            let _ = completion.send(crate::typed_request_outcome::<R::Reply>(outcome));
        });
        (conversion, completion)
    }
}

pub(crate) enum LiveEvent {
    Message(QueuedMessage),
    PortIngress(LivePortIngress),
    SourceEvent {
        stamp: SourceStamp,
        terminal_type: TypeId,
        event: ErasedSourceEvent,
    },
    EffectOutcome {
        effect: u64,
        outcome: ErasedValue,
    },
    TimerFired {
        timer: u64,
        message: QueuedMessage,
    },
    RequestTimedOut {
        correlation: u64,
    },
    Shutdown(Shutdown),
}

impl LiveEvent {
    pub(crate) fn ingress(ingress: LiveIngress) -> Self {
        Self::Message(QueuedMessage {
            target: ingress.target,
            target_message_type: ingress.target_message_type,
            message_type_name: ingress.message_type_name,
            message: ingress.message,
            source: None,
        })
    }

    pub(crate) fn port_notification<P, N>(port: Port<P>, notification: N) -> Self
    where
        P: Protocol,
        N: Notification<P>,
    {
        Self::PortIngress(LivePortIngress::Notification(Box::new(
            TypedLivePortNotification { port, notification },
        )))
    }

    pub(crate) fn port_request<P, R>(
        port: Port<P>,
        request: R,
        deadline: Option<tokio::time::Instant>,
        completion: oneshot::Sender<RequestOutcome<R::Reply>>,
    ) -> Self
    where
        P: Protocol,
        R: Request<P>,
    {
        Self::PortIngress(LivePortIngress::Request(Box::new(TypedLivePortRequest {
            port,
            request,
            deadline,
            completion,
        })))
    }
}

pub(crate) struct QueuedMessage {
    target: ComponentId,
    target_message_type: TypeId,
    message_type_name: &'static str,
    message: ErasedValue,
    source: Option<SourceStamp>,
}

struct ActiveSource {
    stamp: SourceStamp,
    running: bool,
    terminal_observed: bool,
    drain_retained: bool,
    subscription: Box<dyn ErasedRuntimeSubscription>,
    plan: SourcePlan,
    gate: Arc<SourceGate>,
    task: AbortHandle,
}

struct PendingEffect {
    id: u64,
    component: ComponentId,
    descriptor_type_name: &'static str,
    mapper: Option<ErasedMessageMapper>,
    message_type: TypeId,
    message_type_name: &'static str,
    task: AbortHandle,
}

enum RequestContinuation {
    Component {
        requester: ComponentId,
        mapper: ErasedRequestMapper<ErasedValue>,
        message_type: TypeId,
        message_type_name: &'static str,
    },
    Host(HostReplyCompletion),
}

struct OutstandingRequest {
    token: Arc<RequestToken>,
    deadline: Option<tokio::time::Instant>,
    deadline_task: Option<AbortHandle>,
    owner: ComponentId,
    work: TraceId,
    reply_type: TypeId,
    continuation: RequestContinuation,
}

struct PendingTimer {
    id: u64,
    task: AbortHandle,
}

#[derive(Clone)]
enum TaskContext {
    Effect {
        id: u64,
        component: ComponentId,
        descriptor_type: &'static str,
        work: TraceId,
    },
    Source {
        stamp: SourceStamp,
        descriptor_type: &'static str,
        work: TraceId,
    },
    Timer {
        component: ComponentId,
        work: TraceId,
    },
}

struct TaskExit {
    context: TaskContext,
    outcome: Result<Result<(), Arc<str>>, ()>,
}

/// Owned asynchronous engine behind the public live runtime facade.
pub(crate) struct LiveCore {
    program: Program,
    bindings: LiveBindings,
    scope: Arc<LiveScope>,
    receiver: mpsc::UnboundedReceiver<LiveEvent>,
    tasks: JoinSet<TaskExit>,
    sources: Vec<ActiveSource>,
    effects: Vec<PendingEffect>,
    requests: Vec<OutstandingRequest>,
    timers: Vec<PendingTimer>,
    next_generation: u64,
    next_effect: u64,
    next_request: u64,
    next_timer: u64,
    next_work: u64,
    draining: bool,
}

impl LiveCore {
    pub(crate) fn new(
        program: Program,
        bindings: LiveBindings,
        scope: Arc<LiveScope>,
        receiver: mpsc::UnboundedReceiver<LiveEvent>,
    ) -> Self {
        Self {
            program,
            bindings,
            scope,
            receiver,
            tasks: JoinSet::new(),
            sources: Vec::new(),
            effects: Vec::new(),
            requests: Vec::new(),
            timers: Vec::new(),
            next_generation: 0,
            next_effect: 0,
            next_request: 0,
            next_timer: 0,
            next_work: 0,
            draining: false,
        }
    }

    fn work(&mut self) -> TraceId {
        let work = TraceId(self.next_work);
        self.next_work += 1;
        work
    }

    fn runtime_fault(
        &mut self,
        component: ComponentId,
        descriptor: Option<&'static str>,
        work: TraceId,
        reason: impl Into<Arc<str>>,
    ) -> RuntimeError {
        RuntimeError::fault(component, descriptor, work, reason)
    }

    fn component_index(&self, id: &ComponentId, message_type: TypeId) -> Option<usize> {
        self.program.components.iter().position(|component| {
            component.id() == id && component.message_type_id() == message_type
        })
    }

    fn binding_index(&self, protocol: TypeId, port: &PortId) -> Option<usize> {
        self.program
            .bindings
            .iter()
            .position(|binding| binding.protocol_type() == protocol && binding.port_id() == port)
    }

    fn source_index(
        &self,
        component: &ComponentId,
        subscription: &SubscriptionId,
    ) -> Option<usize> {
        self.sources.iter().position(|source| {
            source.stamp.component == *component && source.stamp.subscription == *subscription
        })
    }

    fn source_stamp_is_current(&self, stamp: &SourceStamp) -> bool {
        self.source_index(&stamp.component, &stamp.subscription)
            .is_some_and(|index| self.sources[index].stamp.generation == stamp.generation)
    }

    fn spawn_owned<F>(&mut self, context: TaskContext, future: F) -> AbortHandle
    where
        F: std::future::Future<Output = Result<(), Arc<str>>> + Send + 'static,
    {
        let exit_context = context.clone();
        self.tasks.spawn(async move {
            let outcome = AssertUnwindSafe(future)
                .catch_unwind()
                .await
                .map_err(|_| ());
            TaskExit {
                context: exit_context,
                outcome,
            }
        })
    }

    fn initialize(&mut self) -> Result<(), RuntimeError> {
        self.observe_drain_cutoff()?;
        let mut order = (0..self.program.components.len()).collect::<Vec<_>>();
        order.sort_by(|left, right| {
            self.program.components[*left]
                .id()
                .cmp(self.program.components[*right].id())
        });
        for index in order {
            if self.scope.is_cancelling() {
                return Ok(());
            }
            let component = self.program.components[index].id().clone();
            let work = self.work();
            let (changes, command) = {
                let kernel = &mut self.program.components[index];
                (
                    kernel.reconcile_subscriptions_erased(),
                    kernel.take_initial_command_erased(),
                )
            };
            let changes = changes.map_err(|duplicate| {
                self.runtime_fault(
                    component.clone(),
                    None,
                    work,
                    format!("duplicate desired Subscription identity {duplicate:?}"),
                )
            })?;
            if self.scope.is_cancelling() {
                return Ok(());
            }
            self.observe_drain_cutoff()?;
            self.apply_subscription_changes(&component, changes, work)?;
            if let Some(command) = command {
                if self.scope.is_cancelling() {
                    return Ok(());
                }
                command.interpret_live(self, &component, work)?;
            }
        }
        Ok(())
    }

    fn activate_source(
        &mut self,
        component: &ComponentId,
        subscription: Box<dyn ErasedRuntimeSubscription>,
        work: TraceId,
    ) -> Result<(), RuntimeError> {
        if self.draining || !self.scope.is_running() {
            return Ok(());
        }
        let plan = subscription.source_plan();
        if !plan.accepts_output_event_type(subscription.source_event_type_id()) {
            return Err(self.runtime_fault(
                component.clone(),
                Some(plan.terminal_type_name()),
                work,
                "SourcePlan output does not match its Subscription event type",
            ));
        }
        if !self.program.declares_source_plan(
            plan.capability(),
            subscription.descriptor().type_id(),
            plan.terminal_type_id(),
        ) {
            return Err(self.runtime_fault(
                component.clone(),
                Some(plan.terminal_type_name()),
                work,
                "Source capability belongs to another Program or has inconsistent terminal metadata",
            ));
        }
        let binding = self.bindings.source(&plan).map_err(|reason| {
            self.runtime_fault(
                component.clone(),
                Some(plan.terminal_type_name()),
                work,
                reason,
            )
        })?;
        let stamp = SourceStamp {
            component: component.clone(),
            subscription: subscription.id().clone(),
            generation: self.next_generation,
        };
        self.next_generation += 1;
        let gate = Arc::new(SourceGate::new());
        let sink = Arc::new(SourceSinkCore {
            scope: self.scope.clone(),
            gate: gate.clone(),
            stamp: stamp.clone(),
            terminal_type: plan.terminal_type_id(),
        });
        let context = TaskContext::Source {
            stamp: stamp.clone(),
            descriptor_type: plan.terminal_type_name(),
            work,
        };
        let future = catch_unwind(AssertUnwindSafe(|| {
            binding.run(plan.terminal_descriptor(), sink)
        }))
        .map_err(|_| {
            self.runtime_fault(
                component.clone(),
                Some(plan.terminal_type_name()),
                work,
                "SourceDriver panicked while starting",
            )
        })?;
        let task = self.spawn_owned(context, future);
        self.sources.push(ActiveSource {
            stamp,
            running: true,
            terminal_observed: false,
            drain_retained: false,
            subscription,
            plan,
            gate,
            task,
        });
        Ok(())
    }

    fn stop_source(
        &mut self,
        index: usize,
        retain_for_drain: bool,
        work: TraceId,
    ) -> Result<(), RuntimeError> {
        let (stopped, component, descriptor_type) = {
            let source = &mut self.sources[index];
            let stopped = source.gate.stop();
            source.task.abort();
            source.running = false;
            source.drain_retained |= retain_for_drain;
            (
                stopped,
                source.stamp.component.clone(),
                source.plan.terminal_type_name(),
            )
        };
        stopped.map_err(|reason| self.runtime_fault(component, Some(descriptor_type), work, reason))
    }

    fn apply_subscription_changes(
        &mut self,
        component: &ComponentId,
        mut changes: Vec<ErasedSubscriptionChange>,
        work: TraceId,
    ) -> Result<(), RuntimeError> {
        changes.sort_by(|left, right| left.id().cmp(right.id()));
        for change in changes {
            self.observe_drain_cutoff()?;
            match change {
                ErasedSubscriptionChange::Start(subscription) => {
                    self.activate_source(component, subscription, work)?;
                }
                ErasedSubscriptionChange::Retain(subscription) => {
                    if let Some(index) = self.source_index(component, subscription.id()) {
                        self.sources[index].subscription = subscription;
                    }
                }
                ErasedSubscriptionChange::Replace(subscription) => {
                    let id = subscription.id().clone();
                    if let Some(index) = self.source_index(component, &id) {
                        if self.draining && self.sources[index].drain_retained {
                            continue;
                        }
                        self.stop_source(index, false, work)?;
                        self.sources.remove(index);
                    }
                    self.activate_source(component, subscription, work)?;
                }
                ErasedSubscriptionChange::Cancel(id) => {
                    if let Some(index) = self.source_index(component, &id) {
                        if self.draining && self.sources[index].drain_retained {
                            continue;
                        }
                        self.stop_source(index, false, work)?;
                        self.sources.remove(index);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn interpret_command<Message>(
        &mut self,
        command: Command<Message>,
        component: &ComponentId,
        _cause: TraceId,
    ) -> Result<(), RuntimeError>
    where
        Message: Send + 'static,
    {
        for declaration in command.into_declarations() {
            // This lock-backed check is the issuance boundary for each finite
            // declaration. If it observes Cancel, the declaration is dropped;
            // if it wins first, Cancel may immediately abort the newly issued
            // obligation as ordinary in-flight work.
            if self.scope.is_cancelling() {
                return Ok(());
            }
            self.observe_drain_cutoff()?;
            let work = self.work();
            match declaration.0 {
                CommandKind::None | CommandKind::Batch(_) => {
                    unreachable!("Command::into_declarations removes None and Batch")
                }
                CommandKind::Effect(command) => {
                    let descriptor_type = command.intent().type_id();
                    let descriptor_type_name = command.intent_type_name();
                    if !self
                        .program
                        .declares_effect(command.capability(), descriptor_type)
                    {
                        return Err(self.runtime_fault(
                            component.clone(),
                            Some(descriptor_type_name),
                            work,
                            "Effect capability belongs to another Program",
                        ));
                    }
                    let binding = self.bindings.effect(descriptor_type).ok_or_else(|| {
                        self.runtime_fault(
                            component.clone(),
                            Some(descriptor_type_name),
                            work,
                            "missing live terminal Effect binding",
                        )
                    })?;
                    let (descriptor, mapper) = command.into_parts();
                    let mapper: Option<ErasedMessageMapper> = mapper.map(|mapper| {
                        Box::new(move |outcome| erase_effect_continuation(mapper(outcome)))
                            as ErasedMessageMapper
                    });
                    let id = self.next_effect;
                    self.next_effect += 1;
                    let scope = self.scope.clone();
                    let future = async move {
                        let outcome = binding.execute(descriptor).await;
                        let _ = scope.accept_finite(LiveEvent::EffectOutcome {
                            effect: id,
                            outcome,
                        });
                        Ok(())
                    };
                    let task = self.spawn_owned(
                        TaskContext::Effect {
                            id,
                            component: component.clone(),
                            descriptor_type: descriptor_type_name,
                            work,
                        },
                        future,
                    );
                    self.effects.push(PendingEffect {
                        id,
                        component: component.clone(),
                        descriptor_type_name,
                        mapper,
                        message_type: TypeId::of::<Message>(),
                        message_type_name: std::any::type_name::<Message>(),
                        task,
                    });
                }
                CommandKind::Send(command) => {
                    let target = command.target().clone();
                    let message_type = command.message_type_id();
                    if !Arc::ptr_eq(command.target_program(), &self.program.program)
                        || self.component_index(&target, message_type).is_none()
                    {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            format!("direct send target {target:?} is absent from the Program"),
                        ));
                    }
                    let _ = self.scope.accept_finite(LiveEvent::Message(QueuedMessage {
                        target,
                        target_message_type: message_type,
                        message_type_name: command.message_type_name(),
                        message: command.into_message(),
                        source: None,
                    }));
                }
                CommandKind::Notify(command) => {
                    let port = command.port().clone();
                    let protocol = command.protocol_type_id();
                    if !Arc::ptr_eq(command.port_program(), &self.program.program) {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            "notification Port belongs to another Program",
                        ));
                    }
                    let Some(binding_index) = self.binding_index(protocol, &port) else {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            "notification Port has no runtime binding",
                        ));
                    };
                    let protocol_message = command.into_message();
                    let binding = &self.program.bindings[binding_index];
                    let message = QueuedMessage {
                        target: binding.provider_id().clone(),
                        target_message_type: binding.provider_message_type(),
                        message_type_name: binding.provider_message_type_name(),
                        message: binding.convert(protocol_message),
                        source: None,
                    };
                    let _ = self.scope.accept_finite(LiveEvent::Message(message));
                }
                CommandKind::Request(command) => {
                    let port = command.port().clone();
                    let protocol = command.protocol_type_id();
                    if !Arc::ptr_eq(command.port_program(), &self.program.program) {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            "request Port belongs to another Program",
                        ));
                    }
                    let Some(binding_index) = self.binding_index(protocol, &port) else {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            "request Port has no runtime binding",
                        ));
                    };
                    let deadline = match command.timeout() {
                        Some(timeout) => Some(
                            tokio::time::Instant::now()
                                .checked_add(timeout)
                                .ok_or_else(|| {
                                    self.runtime_fault(
                                        component.clone(),
                                        None,
                                        work,
                                        "request deadline is unrepresentable",
                                    )
                                })?,
                        ),
                        None => None,
                    };
                    let correlation = self.next_request;
                    self.next_request += 1;
                    let reply_type = command.reply_type_id();
                    let token = RequestToken::new(correlation, self.program.program.clone());
                    let (protocol_message, mapper): (_, ErasedRequestMapper<Message>) =
                        command.into_parts(token.clone());
                    let mapper: ErasedRequestMapper<ErasedValue> =
                        Box::new(move |reply| Box::new(mapper(reply)));
                    let binding = &self.program.bindings[binding_index];
                    let message = QueuedMessage {
                        target: binding.provider_id().clone(),
                        target_message_type: binding.provider_message_type(),
                        message_type_name: binding.provider_message_type_name(),
                        message: binding.convert(protocol_message),
                        source: None,
                    };
                    let deadline_task =
                        self.schedule_request_deadline(correlation, deadline, component, work);
                    self.requests.push(OutstandingRequest {
                        token,
                        deadline,
                        deadline_task,
                        owner: component.clone(),
                        work,
                        reply_type,
                        continuation: RequestContinuation::Component {
                            requester: component.clone(),
                            mapper,
                            message_type: TypeId::of::<Message>(),
                            message_type_name: std::any::type_name::<Message>(),
                        },
                    });
                    let _ = self.scope.accept_finite(LiveEvent::Message(message));
                }
                CommandKind::Reply(command) => {
                    let reply_type = command.reply_type_id();
                    let (token, reply) = command.into_parts();
                    if !Arc::ptr_eq(&token.program, &self.program.program) {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            "ReplyTo belongs to another Program",
                        ));
                    }
                    if token.is_expired() {
                        continue;
                    }
                    let Some(index) = self
                        .requests
                        .iter()
                        .position(|request| Arc::ptr_eq(&request.token, &token))
                    else {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            "ReplyTo no longer names an outstanding Request",
                        ));
                    };
                    if self.requests[index].reply_type != reply_type {
                        return Err(self.runtime_fault(
                            component.clone(),
                            None,
                            work,
                            "Reply type does not match its Request correlation",
                        ));
                    }
                    if self.requests[index]
                        .deadline
                        .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
                    {
                        self.complete_request(index, RequestOutcome::TimedOut)?;
                    } else {
                        self.complete_request(index, RequestOutcome::Replied(reply))?;
                    }
                }
                CommandKind::After { delay, message } => {
                    let id = self.next_timer;
                    self.next_timer += 1;
                    let queued = QueuedMessage {
                        target: component.clone(),
                        target_message_type: TypeId::of::<Message>(),
                        message_type_name: std::any::type_name::<Message>(),
                        message: Box::new(message),
                        source: None,
                    };
                    let scope = self.scope.clone();
                    let future = async move {
                        tokio::time::sleep(delay).await;
                        let _ = scope.accept_finite(LiveEvent::TimerFired {
                            timer: id,
                            message: queued,
                        });
                        Ok(())
                    };
                    let task = self.spawn_owned(
                        TaskContext::Timer {
                            component: component.clone(),
                            work,
                        },
                        future,
                    );
                    self.timers.push(PendingTimer { id, task });
                }
            }
        }
        Ok(())
    }

    fn process_message(&mut self, message: QueuedMessage) -> Result<(), RuntimeError> {
        if let Some(stamp) = &message.source
            && !self.source_stamp_is_current(stamp)
        {
            return Ok(());
        }
        let work = self.work();
        let Some(index) = self.component_index(&message.target, message.target_message_type) else {
            return Err(self.runtime_fault(
                message.target,
                None,
                work,
                "Message target is absent or has a different Message type",
            ));
        };
        let command = catch_unwind(AssertUnwindSafe(|| {
            self.program.components[index].transition_erased(message.message)
        }))
        .map_err(|_| {
            self.runtime_fault(
                message.target.clone(),
                None,
                work,
                "Component transition panicked",
            )
        })?
        .map_err(|_| RuntimeError::harness("internal typed Message delivery mismatch"))?;

        self.finish_transition(&message.target, index, work, command)
    }

    /// Applies work projected by an already-committed transition.
    ///
    /// Keeping this boundary explicit makes the Cancel cutoff testable without
    /// putting synchronization or other forbidden behavior inside a Component
    /// update. A transition that was already running may commit its Model, but
    /// Cancel prevents its newly projected Sources and finite Commands from
    /// starting afterward.
    fn finish_transition(
        &mut self,
        target: &ComponentId,
        index: usize,
        work: TraceId,
        command: Box<dyn ErasedCommand>,
    ) -> Result<(), RuntimeError> {
        if self.scope.is_cancelling() {
            return Ok(());
        }
        self.observe_drain_cutoff()?;
        let changes = self.program.components[index]
            .reconcile_subscriptions_erased()
            .map_err(|duplicate| {
                self.runtime_fault(
                    target.clone(),
                    None,
                    work,
                    format!("duplicate desired Subscription identity {duplicate:?}"),
                )
            })?;
        if self.scope.is_cancelling() {
            return Ok(());
        }
        self.observe_drain_cutoff()?;
        self.apply_subscription_changes(target, changes, work)?;
        if self.scope.is_cancelling() {
            return Ok(());
        }
        command.interpret_live(self, target, work)
    }

    fn process_source_event(
        &mut self,
        stamp: SourceStamp,
        terminal_type: TypeId,
        event: ErasedSourceEvent,
    ) -> Result<(), RuntimeError> {
        let Some(index) = self.source_index(&stamp.component, &stamp.subscription) else {
            return Ok(());
        };
        if self.sources[index].stamp.generation != stamp.generation
            || self.sources[index].terminal_observed
            || (!self.sources[index].running && !self.sources[index].drain_retained)
            || self.sources[index].plan.terminal_type_id() != terminal_type
        {
            return Ok(());
        }
        let work = self.work();
        let terminal_name = self.sources[index].plan.terminal_type_name();
        let mapped_result = catch_unwind(AssertUnwindSafe(|| {
            let source = &mut self.sources[index];
            let raw_terminal = event.kind.is_terminal();
            let outer_events = source.plan.map_event(event);
            let outer_terminal =
                raw_terminal || outer_events.iter().any(|event| event.kind.is_terminal());
            let mapped = outer_events
                .into_iter()
                .map(|event| source.subscription.map_event(event))
                .collect::<Vec<_>>();
            (
                source.subscription.message_type_id(),
                source.subscription.message_type_name(),
                mapped,
                outer_terminal,
            )
        }));
        let (message_type, message_type_name, mapped, outer_terminal) =
            mapped_result.map_err(|_| {
                self.runtime_fault(
                    stamp.component.clone(),
                    Some(terminal_name),
                    work,
                    "Source Layer or message mapper panicked",
                )
            })?;
        if outer_terminal {
            self.sources[index].terminal_observed = true;
            self.sources[index].running = false;
            if !self.sources[index].drain_retained {
                let stopped = self.sources[index].gate.stop();
                self.sources[index].task.abort();
                stopped.map_err(|reason| {
                    self.runtime_fault(stamp.component.clone(), Some(terminal_name), work, reason)
                })?;
            }
        }
        for message in mapped {
            let _ = self.scope.accept_finite(LiveEvent::Message(QueuedMessage {
                target: stamp.component.clone(),
                target_message_type: message_type,
                message_type_name,
                message,
                source: Some(stamp.clone()),
            }));
        }
        Ok(())
    }

    fn process_effect(&mut self, id: u64, outcome: ErasedValue) -> Result<(), RuntimeError> {
        let Some(index) = self.effects.iter().position(|effect| effect.id == id) else {
            return Ok(());
        };
        let mut effect = self.effects.remove(index);
        let Some(mapper) = effect.mapper.take() else {
            return Ok(());
        };
        let work = self.work();
        let continuation = catch_unwind(AssertUnwindSafe(|| mapper(outcome))).map_err(|_| {
            self.runtime_fault(
                effect.component.clone(),
                Some(effect.descriptor_type_name),
                work,
                "Effect outcome mapper panicked",
            )
        })?;
        match continuation {
            ErasedEffectContinuation::Message(message) => {
                let _ = self.scope.accept_finite(LiveEvent::Message(QueuedMessage {
                    target: effect.component,
                    target_message_type: effect.message_type,
                    message_type_name: effect.message_type_name,
                    message,
                    source: None,
                }));
            }
            ErasedEffectContinuation::Command(command) => {
                command.interpret_live(self, &effect.component, work)?;
            }
        }
        Ok(())
    }

    fn process_port_ingress(&mut self, ingress: LivePortIngress) -> Result<(), RuntimeError> {
        let work = self.work();
        match ingress {
            LivePortIngress::Notification(notification) => {
                let port = notification.port().clone();
                let protocol_type = notification.protocol_type();
                if !Arc::ptr_eq(notification.port_program(), &self.program.program) {
                    return Err(self.runtime_fault(
                        ComponentId::new("<external-port>"),
                        None,
                        work,
                        "notification Port belongs to another Program",
                    ));
                }
                let Some(binding_index) = self.binding_index(protocol_type, &port) else {
                    return Err(self.runtime_fault(
                        ComponentId::new("<external-port>"),
                        None,
                        work,
                        "notification Port has no runtime binding",
                    ));
                };
                let (provider, target_message_type, message_type_name) = {
                    let binding = &self.program.bindings[binding_index];
                    (
                        binding.provider_id().clone(),
                        binding.provider_message_type(),
                        binding.provider_message_type_name(),
                    )
                };
                let protocol_message =
                    match catch_unwind(AssertUnwindSafe(|| notification.into_message())) {
                        Ok(message) => message,
                        Err(_) => {
                            return Err(self.runtime_fault(
                                provider,
                                None,
                                work,
                                "external Notification protocol conversion panicked",
                            ));
                        }
                    };
                let provider_message = {
                    let binding = &self.program.bindings[binding_index];
                    catch_unwind(AssertUnwindSafe(|| binding.convert(protocol_message)))
                };
                let provider_message = match provider_message {
                    Ok(message) => message,
                    Err(_) => {
                        return Err(self.runtime_fault(
                            provider.clone(),
                            None,
                            work,
                            "external Notification binding conversion panicked",
                        ));
                    }
                };
                let message = QueuedMessage {
                    target: provider,
                    target_message_type,
                    message_type_name,
                    message: provider_message,
                    source: None,
                };
                let _ = self.scope.accept_finite(LiveEvent::Message(message));
            }
            LivePortIngress::Request(request) => {
                let port = request.port().clone();
                let protocol_type = request.protocol_type();
                if !Arc::ptr_eq(request.port_program(), &self.program.program) {
                    return Err(self.runtime_fault(
                        ComponentId::new("<external-port>"),
                        None,
                        work,
                        "request Port belongs to another Program",
                    ));
                }
                let Some(binding_index) = self.binding_index(protocol_type, &port) else {
                    return Err(self.runtime_fault(
                        ComponentId::new("<external-port>"),
                        None,
                        work,
                        "request Port has no runtime binding",
                    ));
                };
                let (provider, target_message_type, message_type_name) = {
                    let binding = &self.program.bindings[binding_index];
                    (
                        binding.provider_id().clone(),
                        binding.provider_message_type(),
                        binding.provider_message_type_name(),
                    )
                };
                let correlation = self.next_request;
                self.next_request += 1;
                let reply_type = request.reply_type();
                let deadline = request.deadline();
                let token = RequestToken::new(correlation, self.program.program.clone());
                let owner = ComponentId::new("<external-port>");
                let deadline_task =
                    self.schedule_request_deadline(correlation, deadline, &owner, work);
                let (conversion, completion) = request.into_conversion();
                self.requests.push(OutstandingRequest {
                    token: token.clone(),
                    deadline,
                    deadline_task,
                    owner,
                    work,
                    reply_type,
                    continuation: RequestContinuation::Host(completion),
                });
                let protocol_message = match catch_unwind(AssertUnwindSafe(|| conversion(token))) {
                    Ok(message) => message,
                    Err(_) => {
                        return Err(self.runtime_fault(
                            provider,
                            None,
                            work,
                            "external Request protocol conversion panicked",
                        ));
                    }
                };
                let provider_message = {
                    let binding = &self.program.bindings[binding_index];
                    catch_unwind(AssertUnwindSafe(|| binding.convert(protocol_message)))
                };
                let provider_message = match provider_message {
                    Ok(message) => message,
                    Err(_) => {
                        return Err(self.runtime_fault(
                            provider.clone(),
                            None,
                            work,
                            "external Request binding conversion panicked",
                        ));
                    }
                };
                let message = QueuedMessage {
                    target: provider,
                    target_message_type,
                    message_type_name,
                    message: provider_message,
                    source: None,
                };
                if !self.scope.accept_finite(LiveEvent::Message(message))
                    && let Some(request) = self.requests.pop()
                    && let Some(task) = request.deadline_task
                {
                    task.abort();
                }
            }
        }
        Ok(())
    }

    fn schedule_request_deadline(
        &mut self,
        correlation: u64,
        deadline: Option<tokio::time::Instant>,
        component: &ComponentId,
        work: TraceId,
    ) -> Option<AbortHandle> {
        deadline.map(|deadline| {
            let scope = self.scope.clone();
            self.spawn_owned(
                TaskContext::Timer {
                    component: component.clone(),
                    work,
                },
                async move {
                    tokio::time::sleep_until(deadline).await;
                    let _ = scope.accept_finite(LiveEvent::RequestTimedOut { correlation });
                    Ok(())
                },
            )
        })
    }

    fn complete_request(
        &mut self,
        index: usize,
        outcome: RequestOutcome<ErasedValue>,
    ) -> Result<(), RuntimeError> {
        let request = self.requests.remove(index);
        if let Some(task) = request.deadline_task {
            task.abort();
        }
        if matches!(outcome, RequestOutcome::TimedOut) {
            request.token.expire();
        }
        match request.continuation {
            RequestContinuation::Component {
                requester,
                mapper,
                message_type,
                message_type_name,
            } => {
                let mapped = catch_unwind(AssertUnwindSafe(|| mapper(outcome))).map_err(|_| {
                    self.runtime_fault(
                        request.owner.clone(),
                        None,
                        request.work,
                        "Request continuation panicked",
                    )
                })?;
                let _ = self.scope.accept_finite(LiveEvent::Message(QueuedMessage {
                    target: requester,
                    target_message_type: message_type,
                    message_type_name,
                    message: mapped,
                    source: None,
                }));
            }
            RequestContinuation::Host(completion) => {
                catch_unwind(AssertUnwindSafe(|| completion(outcome))).map_err(|_| {
                    self.runtime_fault(
                        request.owner,
                        None,
                        request.work,
                        "host Request continuation panicked",
                    )
                })?;
            }
        }
        Ok(())
    }

    fn process_request_timeout(&mut self, correlation: u64) -> Result<(), RuntimeError> {
        if let Some(index) = self
            .requests
            .iter()
            .position(|request| request.token.correlation == correlation)
        {
            self.complete_request(index, RequestOutcome::TimedOut)?;
        }
        Ok(())
    }

    fn begin_drain(&mut self) -> Result<(), RuntimeError> {
        if self.draining {
            return Ok(());
        }
        self.draining = true;
        for index in 0..self.sources.len() {
            let work = self.work();
            self.stop_source(index, true, work)?;
        }
        Ok(())
    }

    fn observe_drain_cutoff(&mut self) -> Result<(), RuntimeError> {
        if !self.draining && self.scope.is_draining() {
            // The host changes scope admission atomically. Observe that phase
            // directly instead of waiting for the queued shutdown marker to
            // work through an arbitrary pre-cutoff backlog. This stops live
            // Source tasks promptly while their already accepted stamped
            // deliveries remain available for causal Drain processing.
            self.begin_drain()?;
        }
        Ok(())
    }

    async fn cleanup_abort(&mut self) {
        for source in &self.sources {
            let _ = source.gate.stop();
        }
        for effect in &self.effects {
            effect.task.abort();
        }
        for timer in &self.timers {
            timer.task.abort();
        }
        self.tasks.abort_all();
        self.sources.clear();
        self.effects.clear();
        self.requests.clear();
        self.timers.clear();
        while self.tasks.join_next().await.is_some() {}
        while self.receiver.try_recv().is_ok() {}
    }

    fn drain_finished(&self) -> bool {
        self.draining
            && self.effects.is_empty()
            && self.requests.is_empty()
            && self.timers.is_empty()
            && self.receiver.is_empty()
            && self.tasks.is_empty()
    }

    fn clean_report() -> ShutdownReport {
        ShutdownReport {
            completed: 0,
            cancelled: 0,
            remaining: 0,
            pending_now: 0,
            pending_later: 0,
        }
    }

    fn retain_pre_cancel_effect_outcome(&mut self, event: LiveEvent) -> Result<(), RuntimeError> {
        if let LiveEvent::EffectOutcome { effect, outcome } = event {
            // An Effect result admitted before Cancel owns its exactly-once
            // mapper invocation even though Cancel discards the resulting
            // application Message and stops further Component driving.
            self.process_effect(effect, outcome)?;
        }
        Ok(())
    }

    async fn finish_cancel(
        &mut self,
        first_event: Option<LiveEvent>,
    ) -> Result<ShutdownReport, RuntimeError> {
        if let Some(event) = first_event
            && let Err(error) = self.retain_pre_cancel_effect_outcome(event)
        {
            self.scope.fault(error.clone());
            self.cleanup_abort().await;
            return Err(error);
        }
        while let Ok(event) = self.receiver.try_recv() {
            if let Err(error) = self.retain_pre_cancel_effect_outcome(event) {
                self.scope.fault(error.clone());
                self.cleanup_abort().await;
                return Err(error);
            }
        }

        // Preserve an already-observed Driver fault rather than allowing a
        // concurrent host Cancel to hide it. Work not yet complete loses the
        // race and is aborted by `cleanup_abort` below.
        while let Some(joined) = self.tasks.try_join_next() {
            match joined {
                Ok(exit) => {
                    if let Some(error) = self.task_fault(exit) {
                        self.scope.fault(error.clone());
                        self.cleanup_abort().await;
                        return Err(error);
                    }
                }
                Err(error) if error.is_cancelled() => {}
                Err(_) => {
                    let work = self.work();
                    let error = self.runtime_fault(
                        ComponentId::new("<runtime>"),
                        None,
                        work,
                        "runtime-owned task failed outside Driver supervision",
                    );
                    self.scope.fault(error.clone());
                    self.cleanup_abort().await;
                    return Err(error);
                }
            }
        }

        // A terminal Source may own synchronous shutdown mechanism outside
        // its async task (stdin's interruptible reader thread is one example).
        // Observe those hooks before aborting tasks so a mechanism failure
        // completed before the Cancel cutoff cannot be hidden by cancellation.
        for index in 0..self.sources.len() {
            let work = self.work();
            if let Err(error) = self.stop_source(index, false, work) {
                self.scope.fault(error.clone());
                self.cleanup_abort().await;
                return Err(error);
            }
        }

        self.cleanup_abort().await;
        self.scope.close();
        Ok(Self::clean_report())
    }

    fn task_fault(&mut self, exit: TaskExit) -> Option<RuntimeError> {
        let (reason, panicked) = match exit.outcome {
            Err(()) => (Arc::<str>::from("runtime-owned Driver task panicked"), true),
            Ok(Err(reason)) => (reason, false),
            Ok(Ok(())) => return None,
        };
        match exit.context {
            TaskContext::Effect {
                component,
                descriptor_type,
                work,
                ..
            } => Some(self.runtime_fault(component, Some(descriptor_type), work, reason)),
            TaskContext::Source {
                stamp,
                descriptor_type,
                work,
            } => {
                let still_relevant = self
                    .source_index(&stamp.component, &stamp.subscription)
                    .is_some_and(|index| {
                        self.sources[index].stamp.generation == stamp.generation
                            && (self.sources[index].running
                                || (self.sources[index].drain_retained
                                    && !self.sources[index].terminal_observed))
                    });
                still_relevant.then(|| {
                    self.runtime_fault(stamp.component, Some(descriptor_type), work, reason)
                })
            }
            TaskContext::Timer { component, work } => Some(self.runtime_fault(
                component,
                None,
                work,
                if panicked {
                    Arc::from("runtime-owned timer task panicked")
                } else {
                    reason
                },
            )),
        }
    }

    pub(crate) async fn run(mut self) -> Result<ShutdownReport, RuntimeError> {
        // A host Cancel request may win before Tokio first polls the
        // owner task. In that case no initial finite work is eligible to start:
        // Cancel is the application-driving cutoff, not merely an event the
        // dispatcher eventually observes.
        if self.scope.is_cancelling() {
            return self.finish_cancel(None).await;
        }
        if let Err(error) = self.initialize() {
            self.scope.fault(error.clone());
            self.cleanup_abort().await;
            return Err(error);
        }
        loop {
            if self.scope.is_cancelling() {
                return self.finish_cancel(None).await;
            }
            if let Err(error) = self.observe_drain_cutoff() {
                self.scope.fault(error.clone());
                self.cleanup_abort().await;
                return Err(error);
            }
            if self.drain_finished() {
                self.sources.clear();
                self.scope.close();
                return Ok(Self::clean_report());
            }
            tokio::select! {
                biased;
                joined = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    match joined {
                        Some(Ok(exit)) => {
                            if let Some(error) = self.task_fault(exit) {
                                self.scope.fault(error.clone());
                                self.cleanup_abort().await;
                                return Err(error);
                            }
                        }
                        Some(Err(error)) if error.is_cancelled() => {}
                        Some(Err(_)) => {
                            let work = self.work();
                            let error = self.runtime_fault(
                                ComponentId::new("<runtime>"),
                                None,
                                work,
                                "runtime-owned task failed outside Driver supervision",
                            );
                            self.scope.fault(error.clone());
                            self.cleanup_abort().await;
                            return Err(error);
                        }
                        None => {}
                    }
                }
                event = self.receiver.recv() => {
                    let Some(event) = event else {
                        let work = self.work();
                        let error = self.runtime_fault(
                            ComponentId::new("<runtime>"),
                            None,
                            work,
                            "live runtime event channel closed unexpectedly",
                        );
                        self.scope.fault(error.clone());
                        self.cleanup_abort().await;
                        return Err(error);
                    };
                    if self.scope.is_cancelling() {
                        return self.finish_cancel(Some(event)).await;
                    }
                    let result = match event {
                        LiveEvent::Message(message) => self.process_message(message),
                        LiveEvent::PortIngress(ingress) => self.process_port_ingress(ingress),
                        LiveEvent::SourceEvent { stamp, terminal_type, event } => {
                            self.process_source_event(stamp, terminal_type, event)
                        }
                        LiveEvent::EffectOutcome { effect, outcome } => {
                            self.process_effect(effect, outcome)
                        }
                        LiveEvent::TimerFired { timer, message } => {
                            if let Some(index) = self.timers.iter().position(|pending| pending.id == timer) {
                                self.timers.remove(index);
                                self.process_message(message)
                            } else {
                                Ok(())
                            }
                        }
                        LiveEvent::RequestTimedOut { correlation } => self.process_request_timeout(correlation),
                        LiveEvent::Shutdown(Shutdown::Drain) => self.begin_drain(),
                        LiveEvent::Shutdown(Shutdown::Cancel) => {
                            return self.finish_cancel(None).await;
                        }
                    };
                    if let Err(error) = result {
                        self.scope.fault(error.clone());
                        self.cleanup_abort().await;
                        return Err(error);
                    }
                }
            }
        }
    }
}

/// Creates a first-party exact one-shot mpsc binding.
pub(crate) fn bind_mpsc<S>(
    bindings: &mut LiveBindings,
    capability: &SourceCapability<S>,
    receiver: mpsc::Receiver<S::StreamItem>,
) where
    S: StreamSourceDescriptor,
{
    bindings.bind_mpsc(capability, receiver);
}

/// Requests the first-party stdin binding, resolved during live build.
#[cfg(unix)]
pub(crate) fn bind_stdin(bindings: &mut LiveBindings) {
    bindings.bind_stdin();
}

/// Registers the first-party Tokio TCP terminal Driver.
pub(crate) fn bind_tcp(bindings: &mut LiveBindings) {
    bindings.bind_tcp();
}

/// Registers the first-party pooled HTTP terminal Driver.
pub(crate) fn bind_http(bindings: &mut LiveBindings) {
    bindings.bind_http();
}

/// Registers the first-party Tokio standard-output terminal Drivers.
pub(crate) fn bind_stdio(bindings: &mut LiveBindings) {
    bindings.bind_stdio();
}

/// Proves the first-party descriptor's item type without exposing machinery.
fn _tcp_item_is_bytes(_: SourceEvent<Bytes, TcpError>) {}

/// Marker used only to keep exhaustive type relationships explicit.
fn _stream_error_is_infallible<T>(_: SourceEvent<T, Infallible>) {}

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        future::pending,
        io,
        pin::Pin,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll},
    };

    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

    use super::*;
    use crate::{
        Component, Decoder, EffectDescriptor, Framed, FramedError, Init, Subscription,
        Subscriptions,
    };

    #[derive(Debug)]
    struct CutoffIntent;

    impl EffectDescriptor for CutoffIntent {
        type Output = ();
        type Error = Infallible;
    }

    enum CutoffMessage {
        Start,
        Outcome,
    }

    struct CutoffComponent {
        effect: crate::EffectCapability<CutoffIntent>,
    }

    impl Component for CutoffComponent {
        type Model = bool;
        type Message = CutoffMessage;

        fn init(&self) -> Init<Self::Model, Self::Message> {
            Init::new(false)
        }

        fn update(
            &self,
            started: &mut Self::Model,
            message: Self::Message,
        ) -> Command<Self::Message> {
            match message {
                CutoffMessage::Start => {
                    *started = true;
                    Command::effect_with(&self.effect, CutoffIntent, |_| CutoffMessage::Outcome)
                }
                CutoffMessage::Outcome => Command::none(),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct GenerationSource(u8);

    impl SourceDescriptor for GenerationSource {
        type Item = u64;
        type Error = Infallible;
    }

    enum GenerationMessage {
        Event(SourceEvent<u64, Infallible>),
        Replace(u8),
    }

    #[derive(Default)]
    struct GenerationModel {
        generation: u8,
        items: Vec<u64>,
    }

    struct GenerationComponent {
        source: crate::SourceCapability<GenerationSource>,
    }

    impl Component for GenerationComponent {
        type Model = GenerationModel;
        type Message = GenerationMessage;

        fn init(&self) -> Init<Self::Model, Self::Message> {
            Init::new(GenerationModel {
                generation: 1,
                items: Vec::new(),
            })
        }

        fn update(
            &self,
            model: &mut Self::Model,
            message: Self::Message,
        ) -> Command<Self::Message> {
            match message {
                GenerationMessage::Event(SourceEvent::Item(item)) => model.items.push(item),
                GenerationMessage::Event(SourceEvent::Ended) => {}
                GenerationMessage::Event(SourceEvent::Failed(never)) => match never {},
                GenerationMessage::Replace(generation) => model.generation = generation,
            }
            Command::none()
        }

        fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
            Subscriptions::one(Subscription::source_with(
                &self.source,
                SubscriptionId::new("generation"),
                GenerationSource(model.generation),
                GenerationMessage::Event,
            ))
        }
    }

    struct CapturingSourceDriver {
        sinks: Arc<Mutex<Vec<Arc<SourceSink<GenerationSource>>>>>,
    }

    #[derive(Clone)]
    struct DrainFailDecoder {
        finish_calls: Arc<AtomicUsize>,
    }

    impl std::fmt::Debug for DrainFailDecoder {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("DrainFailDecoder")
        }
    }

    impl PartialEq for DrainFailDecoder {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl Decoder for DrainFailDecoder {
        type Chunk = u64;
        type Frame = u64;
        type Error = &'static str;
        type State = ();

        fn start(&self) -> Self::State {}

        fn push(
            &self,
            _state: &mut Self::State,
            _chunk: Self::Chunk,
        ) -> Result<Vec<Self::Frame>, Self::Error> {
            Err("decode failed")
        }

        fn finish(&self, _state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
            self.finish_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    struct FramedDrainComponent {
        source: crate::SourceCapability<Framed<GenerationSource, DrainFailDecoder>>,
        decoder: DrainFailDecoder,
    }

    impl Component for FramedDrainComponent {
        type Model = ();
        type Message = SourceEvent<u64, FramedError<Infallible, &'static str>>;

        fn init(&self) -> Init<Self::Model, Self::Message> {
            Init::new(())
        }

        fn update(
            &self,
            _model: &mut Self::Model,
            _message: Self::Message,
        ) -> Command<Self::Message> {
            Command::none()
        }

        fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
            Subscriptions::one(Subscription::source_with(
                &self.source,
                SubscriptionId::new("framed-drain"),
                Framed::new(GenerationSource(1), self.decoder.clone()),
                std::convert::identity,
            ))
        }
    }

    impl SourceDriver<GenerationSource> for CapturingSourceDriver {
        fn run(
            &self,
            _descriptor: GenerationSource,
            sink: SourceSink<GenerationSource>,
        ) -> BoxFuture<()> {
            lock(&self.sinks).push(Arc::new(sink));
            Box::pin(pending())
        }
    }

    struct PanickingSourceAtCutoff {
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    impl SourceDriver<GenerationSource> for PanickingSourceAtCutoff {
        fn run(
            &self,
            _descriptor: GenerationSource,
            _sink: SourceSink<GenerationSource>,
        ) -> BoxFuture<()> {
            let entered = lock(&self.entered).take();
            Box::pin(async move {
                if let Some(entered) = entered {
                    let _ = entered.send(());
                }
                panic!("Source panic completed before Drain cutoff");
            })
        }
    }

    struct GenerationFixture {
        core: LiveCore,
        scope: Arc<LiveScope>,
        id: ComponentId,
        index: usize,
        sinks: Arc<Mutex<Vec<Arc<SourceSink<GenerationSource>>>>>,
    }

    fn generation_core() -> GenerationFixture {
        let mut builder = Program::builder();
        let source = builder.source::<GenerationSource>();
        let component = builder.component(
            ComponentId::new("generation"),
            GenerationComponent { source },
        );
        let program = builder.build().expect("valid generation Program");
        let id = component.id().clone();
        let sinks = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = LiveBindings::new();
        bindings.bind_source::<GenerationSource, _>(CapturingSourceDriver {
            sinks: sinks.clone(),
        });
        let (sender, receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let mut core = LiveCore::new(program, bindings, scope.clone(), receiver);
        let index = core
            .component_index(&id, TypeId::of::<GenerationMessage>())
            .expect("registered generation Component");
        core.initialize().expect("initial Source activates");
        GenerationFixture {
            core,
            scope,
            id,
            index,
            sinks,
        }
    }

    struct FailingReader;

    impl AsyncRead for FailingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "deterministic read failure",
            )))
        }
    }

    struct FailingWriter;

    impl AsyncWrite for FailingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "deterministic output failure",
            )))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn phase6_cancel_cutoff_starts_no_work_from_a_committed_transition() {
        let mut builder = Program::builder();
        let effect = builder.effect::<CutoffIntent>();
        let component = builder.component(
            ComponentId::new("cancel-cutoff"),
            CutoffComponent { effect },
        );
        let program = builder.build().expect("valid cutoff Program");
        let id = component.id().clone();
        let (sender, receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let mut core = LiveCore::new(program, LiveBindings::new(), scope.clone(), receiver);
        let index = core
            .component_index(&id, TypeId::of::<CutoffMessage>())
            .expect("registered cutoff Component");
        let work = core.work();

        // Split the runtime's post-transition boundary explicitly: the pure
        // update has committed, then the host Cancel wins before projected
        // work is applied. No synchronization is hidden inside Component code.
        let command = core.program.components[index]
            .transition_erased(Box::new(CutoffMessage::Start))
            .expect("typed transition");
        scope
            .begin_shutdown(Shutdown::Cancel)
            .expect("Cancel cutoff accepted");
        core.finish_transition(&id, index, work, command)
            .expect("post-transition cutoff is ordinary");

        assert!(core.effects.is_empty(), "post-cutoff Effect must not start");
        let model = core.program.components[index]
            .model_any()
            .downcast_ref::<bool>()
            .expect("typed cutoff Model");
        assert!(*model, "the already-running transition still committed");
        assert!(
            core.finish_cancel(None)
                .await
                .expect("clean Cancel")
                .is_clean()
        );
    }

    #[tokio::test]
    async fn phase6_replaced_generation_drops_an_already_accepted_old_event() {
        let GenerationFixture {
            mut core,
            id,
            index,
            sinks,
            ..
        } = generation_core();
        let old = lock(&sinks)
            .first()
            .cloned()
            .expect("initial Source sink captured");
        old.emit(99)
            .await
            .expect("old event accepted before cutover");

        // Commit and apply replacement before consuming the already-queued old
        // event. Its generation stamp must make it stale at delivery time.
        let work = core.work();
        let command = core.program.components[index]
            .transition_erased(Box::new(GenerationMessage::Replace(2)))
            .expect("typed replacement transition");
        core.finish_transition(&id, index, work, command)
            .expect("replacement applies");
        assert_eq!(lock(&sinks).len(), 2, "replacement Source activated");

        let LiveEvent::SourceEvent {
            stamp,
            terminal_type,
            event,
        } = core
            .receiver
            .try_recv()
            .expect("accepted old event remains queued")
        else {
            panic!("old Source acceptance queued a non-Source event");
        };
        core.process_source_event(stamp, terminal_type, event)
            .expect("stale delivery is suppressed");
        assert!(
            core.receiver.is_empty(),
            "stale event must not enqueue a Component Message"
        );
        let model = core.program.components[index]
            .model_any()
            .downcast_ref::<GenerationModel>()
            .expect("typed generation Model");
        assert_eq!(model.generation, 2);
        assert!(model.items.is_empty());
        core.cleanup_abort().await;
    }

    #[tokio::test]
    async fn phase6_drain_cutoff_stops_sources_before_queued_backlog() {
        let GenerationFixture {
            mut core,
            scope,
            sinks,
            ..
        } = generation_core();
        let old = lock(&sinks)
            .first()
            .cloned()
            .expect("initial Source sink captured");
        old.emit(1)
            .await
            .expect("pre-cutoff Source delivery accepted");
        scope
            .begin_shutdown(Shutdown::Drain)
            .expect("Drain cutoff accepted");

        // The Source event and shutdown marker are both still queued. The
        // owner observes scope phase directly, so it aborts the live Source
        // without waiting for either item to traverse that backlog.
        assert!(!core.draining);
        assert_eq!(core.receiver.len(), 2);
        core.observe_drain_cutoff().expect("Drain starts cleanly");
        assert!(core.draining);
        assert!(!core.sources[0].running);
        assert!(core.sources[0].drain_retained);
        assert_eq!(core.receiver.len(), 2, "backlog was not driven first");
        assert_eq!(old.end().await, Err(DriverStopped));
        core.cleanup_abort().await;
    }

    #[tokio::test]
    async fn phase6_drain_does_not_finalize_after_an_earlier_decode_failure() {
        let finish_calls = Arc::new(AtomicUsize::new(0));
        let mut builder = Program::builder();
        let source = builder.source::<Framed<GenerationSource, DrainFailDecoder>>();
        let _ = builder.component(
            ComponentId::new("framed-drain"),
            FramedDrainComponent {
                source,
                decoder: DrainFailDecoder {
                    finish_calls: finish_calls.clone(),
                },
            },
        );
        let program = builder.build().expect("valid framed Drain Program");
        let sinks = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = LiveBindings::new();
        bindings.bind_source::<GenerationSource, _>(CapturingSourceDriver {
            sinks: sinks.clone(),
        });
        let (sender, receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let mut core = LiveCore::new(program, bindings, scope.clone(), receiver);
        core.initialize().expect("framed Source activates");
        let sink = lock(&sinks)
            .first()
            .cloned()
            .expect("terminal Source sink captured");

        sink.emit(1)
            .await
            .expect("decode-failing item accepted before cutoff");
        sink.end()
            .await
            .expect("EOF accepted behind the item before cutoff");
        scope
            .begin_shutdown(Shutdown::Drain)
            .expect("Drain cutoff accepted");
        core.observe_drain_cutoff().expect("Drain starts cleanly");

        for expected in ["decode-failing item", "queued EOF"] {
            let LiveEvent::SourceEvent {
                stamp,
                terminal_type,
                event,
            } = core.receiver.try_recv().expect(expected)
            else {
                panic!("{expected} was not a Source event");
            };
            core.process_source_event(stamp, terminal_type, event)
                .expect("retained raw event processing is valid");
        }

        assert_eq!(
            finish_calls.load(Ordering::SeqCst),
            0,
            "an earlier decode failure is terminal and suppresses finalization"
        );
        core.cleanup_abort().await;
    }

    #[tokio::test]
    async fn phase6_drain_preserves_a_source_panic_completed_before_cutoff() {
        let mut builder = Program::builder();
        let source = builder.source::<GenerationSource>();
        let component = builder.component(
            ComponentId::new("drain-panic"),
            GenerationComponent { source },
        );
        let program = builder.build().expect("valid Source panic Program");
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let mut bindings = LiveBindings::new();
        bindings.bind_source::<GenerationSource, _>(PanickingSourceAtCutoff {
            entered: Mutex::new(Some(entered)),
        });
        let (sender, receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let mut core = LiveCore::new(program, bindings, scope.clone(), receiver);
        core.initialize().expect("Source task starts");
        entered_rx
            .await
            .expect("Source reached its deterministic panic");

        scope
            .begin_shutdown(Shutdown::Drain)
            .expect("Drain cutoff accepted");
        core.observe_drain_cutoff().expect("Drain starts cleanly");
        let exit = core
            .tasks
            .join_next()
            .await
            .expect("Source task exit")
            .expect("panic was supervised inside TaskExit");
        let error = core
            .task_fault(exit)
            .expect("pre-cutoff Source panic remains a runtime fault");
        assert_eq!(error.component(), Some(component.id()));
        assert_eq!(
            error.descriptor_type(),
            Some(std::any::type_name::<GenerationSource>())
        );
        core.cleanup_abort().await;
    }

    #[tokio::test]
    async fn shutdown_escalation_preserves_source_fault_completed_before_cancel() {
        for observe_drain in [false, true] {
            let mut builder = Program::builder();
            let source = builder.source::<GenerationSource>();
            let component = builder.component(
                ComponentId::new("escalation-panic"),
                GenerationComponent { source },
            );
            let (entered, entered_rx) = tokio::sync::oneshot::channel();
            let mut bindings = LiveBindings::new();
            bindings.bind_source::<GenerationSource, _>(PanickingSourceAtCutoff {
                entered: Mutex::new(Some(entered)),
            });
            let (sender, receiver) = mpsc::unbounded_channel();
            let scope = Arc::new(LiveScope::new(sender));
            let mut core = LiveCore::new(
                builder.build().expect("valid program"),
                bindings,
                scope.clone(),
                receiver,
            );
            core.initialize().expect("Source starts");
            entered_rx.await.expect("Source completed its panic");

            scope
                .begin_shutdown(Shutdown::Drain)
                .expect("Drain accepted");
            if observe_drain {
                core.observe_drain_cutoff()
                    .expect("Sources stopped for Drain");
            }
            scope
                .begin_shutdown(Shutdown::Cancel)
                .expect("Cancel escalates");
            assert!(scope.is_cancelling());
            let error = core
                .finish_cancel(None)
                .await
                .expect_err("escalation must preserve the completed Source fault");
            assert_eq!(error.component(), Some(component.id()));
            assert_eq!(
                error.descriptor_type(),
                Some(std::any::type_name::<GenerationSource>())
            );
            assert!(core.tasks.is_empty());
            assert!(core.sources.is_empty());
            assert_eq!(scope.request_ended_error(), error);
        }
    }

    #[tokio::test]
    async fn phase6_timer_task_panic_is_a_runtime_fault() {
        let (sender, receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let program = Program::builder()
            .build()
            .expect("empty timer test Program");
        let mut core = LiveCore::new(program, LiveBindings::new(), scope, receiver);
        let component = ComponentId::new("timer-owner");
        let work = TraceId(17);
        core.spawn_owned(
            TaskContext::Timer {
                component: component.clone(),
                work,
            },
            async {
                panic!("simulated Tokio timer panic");
                #[allow(unreachable_code)]
                Ok(())
            },
        );

        let exit = core
            .tasks
            .join_next()
            .await
            .expect("timer task exit")
            .expect("timer panic was caught by supervision");
        let error = core
            .task_fault(exit)
            .expect("timer panic must fault instead of leaving pending work");
        assert_eq!(error.component(), Some(&component));
        assert_eq!(error.work_occurrence(), Some(work));
        assert!(error.to_string().contains("timer task panicked"));
        core.cleanup_abort().await;
    }

    #[tokio::test]
    async fn standard_output_driver_writes_the_exact_payload() {
        let expected = Bytes::from_static(b"one complete record\n");
        let (writer, mut reader) = tokio::io::duplex(expected.len());

        write_and_flush(Arc::new(tokio::sync::Mutex::new(writer)), expected.clone())
            .await
            .expect("write and flush succeed");

        let mut actual = vec![0; expected.len()];
        reader
            .read_exact(&mut actual)
            .await
            .expect("complete payload available");
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn standard_output_driver_discards_host_io_errors() {
        print_best_effort(
            Arc::new(tokio::sync::Mutex::new(FailingWriter)),
            Bytes::from_static(b"best effort"),
        )
        .await
        .expect("high-level print has no application error");
    }

    #[tokio::test]
    async fn phase6_tcp_read_error_maps_to_typed_source_failure() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let sink = SourceSink::runtime(Arc::new(SourceSinkCore {
            scope,
            gate: Arc::new(SourceGate::new()),
            stamp: SourceStamp {
                component: ComponentId::new("tcp-read-error"),
                subscription: SubscriptionId::new("tcp"),
                generation: 1,
            },
            terminal_type: TypeId::of::<TcpBytes>(),
        }));

        drive_tcp_reader(&mut FailingReader, sink).await;

        let LiveEvent::SourceEvent { event, .. } =
            receiver.recv().await.expect("one terminal Source event")
        else {
            panic!("TCP reader emitted a non-Source event");
        };
        let event = *event
            .value
            .downcast::<SourceEvent<Bytes, TcpError>>()
            .expect("typed TCP Source event");
        let SourceEvent::Failed(error) = event else {
            panic!("read error must be a typed Source failure");
        };
        assert_eq!(error.kind(), crate::TcpErrorKind::Read);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn phase6_cancel_maps_only_effect_outcomes_accepted_before_cutoff() {
        let (sender, receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let program = crate::Program::builder()
            .build()
            .expect("empty test Program is valid");
        let mut core = LiveCore::new(program, LiveBindings::new(), scope.clone(), receiver);
        let task = core.spawn_owned(
            TaskContext::Effect {
                id: 7,
                component: ComponentId::new("cancel-race"),
                descriptor_type: "CancelRaceEffect",
                work: TraceId(0),
            },
            async { pending().await },
        );
        let mapper_calls = Arc::new(AtomicUsize::new(0));
        let calls = mapper_calls.clone();
        core.effects.push(PendingEffect {
            id: 7,
            component: ComponentId::new("cancel-race"),
            descriptor_type_name: "CancelRaceEffect",
            mapper: Some(Box::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                ErasedEffectContinuation::Message(Box::new(()))
            })),
            message_type: TypeId::of::<()>(),
            message_type_name: std::any::type_name::<()>(),
            task,
        });

        // This invalid queued Message proves Cancel does not continue normal
        // application driving. The real Effect result behind it was accepted
        // first, so its one-shot mapper must nevertheless be consumed once.
        assert!(scope.accept_finite(LiveEvent::Message(QueuedMessage {
            target: ComponentId::new("must-not-drive"),
            target_message_type: TypeId::of::<()>(),
            message_type_name: std::any::type_name::<()>(),
            message: Box::new(()),
            source: None,
        })));
        assert!(scope.accept_finite(LiveEvent::EffectOutcome {
            effect: 7,
            outcome: Box::new(()),
        }));
        scope
            .begin_shutdown(Shutdown::Cancel)
            .expect("Cancel cutoff accepted");

        let report = core
            .finish_cancel(None)
            .await
            .expect("selective Cancel cleanup");
        assert!(report.is_clean());
        assert_eq!(mapper_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn phase6_cancel_preserves_a_synchronous_source_stop_failure() {
        let GenerationFixture {
            mut core,
            scope,
            id,
            ..
        } = generation_core();
        assert!(
            core.sources[0]
                .gate
                .install_stop_hook(|| Err(Arc::from("fixture Source stop mechanism failed")))
                .expect("install fixture stop hook")
        );
        scope
            .begin_shutdown(Shutdown::Cancel)
            .expect("Cancel cutoff accepted");

        let error = core
            .finish_cancel(None)
            .await
            .expect_err("a completed stop failure must win over Cancel cleanup");

        assert_eq!(error.component(), Some(&id));
        assert_eq!(
            error.descriptor_type(),
            Some(std::any::type_name::<GenerationSource>())
        );
        assert!(error.to_string().contains("stop mechanism failed"));
    }

    #[tokio::test]
    async fn phase6_mpsc_receiver_is_consumed_at_activation_not_first_poll() {
        let mut builder = crate::Program::builder();
        let capability = builder.source::<StreamDescriptor<u8>>();
        let descriptor = StreamDescriptor::<u8>::named("activation/one-shot");
        let (_sender, receiver) = mpsc::channel::<u8>(1);
        let binding = MpscBinding {
            capability: capability.token,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        };
        let (sender, _events) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let sink = |generation| {
            Arc::new(SourceSinkCore {
                scope: scope.clone(),
                gate: Arc::new(SourceGate::new()),
                stamp: SourceStamp {
                    component: ComponentId::new("one-shot"),
                    subscription: SubscriptionId::new("stream"),
                    generation,
                },
                terminal_type: TypeId::of::<StreamDescriptor<u8>>(),
            })
        };

        // Constructing the first realization future is the activation point;
        // deliberately never poll it before attempting a second realization.
        let first = binding.run(&descriptor, sink(1));
        let second = binding.run(&descriptor, sink(2));
        let error = second
            .await
            .expect_err("the unique Receiver was consumed by first activation");
        assert!(error.contains("already consumed"));
        drop(first);
    }

    #[tokio::test]
    async fn phase6_source_sink_enqueues_fifo_through_one_terminal() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let sink = SourceSink::<StreamDescriptor<u8>>::runtime(Arc::new(SourceSinkCore {
            scope,
            gate: Arc::new(SourceGate::new()),
            stamp: SourceStamp {
                component: ComponentId::new("source-fifo"),
                subscription: SubscriptionId::new("stream"),
                generation: 1,
            },
            terminal_type: TypeId::of::<StreamDescriptor<u8>>(),
        }));

        sink.emit(1).await.expect("first item accepted");
        sink.emit(2).await.expect("second item accepted");
        sink.end().await.expect("terminal accepted");
        assert_eq!(sink.emit(3).await, Err(DriverStopped));
        assert_eq!(sink.end().await, Err(DriverStopped));

        let mut events = Vec::new();
        for _ in 0..3 {
            let LiveEvent::SourceEvent { event, .. } =
                receiver.recv().await.expect("accepted Source event")
            else {
                panic!("SourceSink emitted a non-Source event");
            };
            events.push(
                *event
                    .value
                    .downcast::<SourceEvent<u8, Infallible>>()
                    .expect("typed stream event"),
            );
        }
        assert_eq!(
            events,
            vec![
                SourceEvent::Item(1),
                SourceEvent::Item(2),
                SourceEvent::Ended
            ]
        );
        assert!(receiver.try_recv().is_err());
    }

    #[cfg(unix)]
    fn stdin_test_sink() -> (Arc<SourceSinkCore>, mpsc::UnboundedReceiver<LiveEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let scope = Arc::new(LiveScope::new(sender));
        let sink = Arc::new(SourceSinkCore {
            scope,
            gate: Arc::new(SourceGate::new()),
            stamp: SourceStamp {
                component: ComponentId::new("stdin-reader"),
                subscription: SubscriptionId::new("stdin"),
                generation: 1,
            },
            terminal_type: TypeId::of::<StdinLines>(),
        });
        (sink, receiver)
    }

    #[cfg(unix)]
    fn take_stdin_events(
        receiver: &mut mpsc::UnboundedReceiver<LiveEvent>,
    ) -> Vec<SourceEvent<String, StdinError>> {
        let mut typed = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            let LiveEvent::SourceEvent { event, .. } = event else {
                panic!("stdin Source emitted a non-Source event");
            };
            typed.push(
                *event
                    .value
                    .downcast::<SourceEvent<String, StdinError>>()
                    .expect("typed stdin event"),
            );
        }
        typed
    }

    #[cfg(unix)]
    #[test]
    fn stdin_line_decoder_handles_chunking_crlf_empty_and_unterminated_lines() {
        let mut decoder = StdinLineDecoder::default();
        for _ in 0..32 {
            assert!(decoder.push(b"long chunk ").is_empty());
        }
        assert_eq!(
            decoder.push(b"\n"),
            vec![StdinReaderEvent::Line("long chunk ".repeat(32))]
        );
        assert!(decoder.push(b"first\r").is_empty());
        assert_eq!(
            decoder.push(b"\n\nunterminated"),
            vec![
                StdinReaderEvent::Line("first".to_owned()),
                StdinReaderEvent::Line(String::new()),
            ]
        );
        assert_eq!(
            decoder.finish(),
            vec![
                StdinReaderEvent::Line("unterminated".to_owned()),
                StdinReaderEvent::Ended,
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_reader_delivers_framed_lines_then_eof_from_injected_stream() {
        use std::{io::Write as _, sync::atomic::AtomicBool};

        let active = Arc::new(AtomicBool::new(true));
        let (input, mut world) = UnixStream::pair().expect("injected stdin stream");
        let reader = StdinReader::spawn(input, active.clone()).expect("stdin reader");
        let (sink, mut events) = stdin_test_sink();
        world
            .write_all(b"first\r\n\nlast without newline")
            .expect("write injected stdin");
        world
            .shutdown(std::net::Shutdown::Write)
            .expect("end injected stdin");

        run_stdin_reader(reader, sink)
            .await
            .expect("reader ended normally");

        assert_eq!(
            take_stdin_events(&mut events),
            vec![
                SourceEvent::Item("first".to_owned()),
                SourceEvent::Item(String::new()),
                SourceEvent::Item("last without newline".to_owned()),
                SourceEvent::Ended,
            ]
        );
        assert!(!active.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_reader_stops_at_invalid_utf8_without_synthesizing_eof() {
        use std::{io::Write as _, sync::atomic::AtomicBool};

        let active = Arc::new(AtomicBool::new(true));
        let (input, mut world) = UnixStream::pair().expect("injected stdin stream");
        let reader = StdinReader::spawn(input, active.clone()).expect("stdin reader");
        let (sink, mut events) = stdin_test_sink();
        world
            .write_all(b"valid\n\xff\nignored\n")
            .expect("write invalid injected stdin");
        world
            .shutdown(std::net::Shutdown::Write)
            .expect("end injected stdin");

        run_stdin_reader(reader, sink)
            .await
            .expect("typed failure is not a Driver fault");

        let events = take_stdin_events(&mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], SourceEvent::Item("valid".to_owned()));
        let SourceEvent::Failed(error) = &events[1] else {
            panic!("invalid UTF-8 must terminate with a typed failure");
        };
        assert_eq!(error.kind(), crate::StdinErrorKind::InvalidUtf8);
        assert!(!active.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_reader_reports_private_mechanism_failure_as_runtime_work_failure() {
        let (sender, events) = mpsc::unbounded_channel();
        drop(sender);
        let reader = StdinReader {
            events,
            control: Arc::new(StdinReaderControl {
                state: Mutex::new(StdinReaderControlState::Stopped(Err(Arc::from(
                    "fixture cancellation channel failure",
                )))),
                stopped: Condvar::new(),
            }),
        };
        let (sink, mut emitted) = stdin_test_sink();

        let error = run_stdin_reader(reader, sink)
            .await
            .expect_err("private mechanism failure must leave the Source task");

        assert_eq!(&*error, "fixture cancellation channel failure");
        assert!(
            emitted.try_recv().is_err(),
            "mechanism failure must not masquerade as a typed Source failure"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stdin_cutover_is_normal_when_reader_and_owner_run_concurrently() {
        let active = Arc::new(AtomicBool::new(true));
        let (input, _world) = UnixStream::pair().expect("injected stdin stream");
        let reader = StdinReader::spawn(input, active.clone()).expect("stdin reader");
        let (sink, mut emitted) = stdin_test_sink();
        let gate = sink.gate.clone();
        let control = reader.control.clone();
        assert!(
            gate.install_stop_hook(move || control.stop_and_join())
                .expect("install synchronous stdin cutover")
        );
        let task = tokio::spawn(run_stdin_reader(reader, sink));
        tokio::task::yield_now().await;

        gate.stop().expect("normal cutover joins the reader");
        task.await
            .expect("stdin Source task remains supervised")
            .expect("normal cutover is not a mechanism failure");

        assert!(!active.load(Ordering::Acquire));
        assert!(
            emitted.try_recv().is_err(),
            "cutover must not fabricate a terminal Source event"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dropping_idle_stdin_reader_interrupts_and_joins_its_thread() {
        use std::{
            sync::{atomic::AtomicBool, mpsc as std_mpsc},
            time::{Duration, Instant},
        };

        let active = Arc::new(AtomicBool::new(true));
        let (input, world) = UnixStream::pair().expect("injected stdin stream");
        let reader = StdinReader::spawn(input, active.clone()).expect("stdin reader");
        let (finished, rescue) = std_mpsc::channel();
        let rescue_thread = thread::spawn(move || {
            if rescue.recv_timeout(Duration::from_secs(2)).is_err() {
                let _ = world.shutdown(std::net::Shutdown::Both);
            }
        });

        let started = Instant::now();
        drop(reader);
        let elapsed = started.elapsed();
        let _ = finished.send(());
        rescue_thread.join().expect("cancellation watchdog");

        assert!(
            elapsed < Duration::from_secs(1),
            "idle reader took {elapsed:?} to cancel"
        );
        assert!(!active.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[test]
    fn stdin_binding_rejects_concurrent_then_allows_sequential_realization() {
        let mut builder = crate::Program::builder();
        let capability = builder.source::<StdinLines>();
        let active = Arc::new(AtomicBool::new(false));
        let binding = StdinBinding {
            capability: capability.token.clone(),
            active: active.clone(),
        };
        let competing_binding = StdinBinding {
            capability: capability.token,
            active,
        };
        let (first_input, _first_world) = UnixStream::pair().expect("first stdin stream");
        let first = binding.acquire(first_input).expect("first realization");
        let (second_input, _second_world) = UnixStream::pair().expect("second stdin stream");
        let error = match competing_binding.acquire(second_input) {
            Ok(_) => panic!("concurrent realization must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("already has an active Source"));

        let gate = SourceGate::new();
        let control = first.control.clone();
        assert!(
            gate.install_stop_hook(move || control.stop_and_join())
                .expect("install synchronous stdin cutover")
        );
        gate.stop()
            .expect("Source cutover joins and releases first realization");
        let (third_input, _third_world) = UnixStream::pair().expect("third stdin stream");
        let third = competing_binding
            .acquire(third_input)
            .expect("sequential realization after release");

        // Dropping the already-released first guard must not clear the newer
        // realization's process lease.
        drop(first);
        let (fourth_input, _fourth_world) = UnixStream::pair().expect("fourth stdin stream");
        let error = match binding.acquire(fourth_input) {
            Ok(_) => panic!("an old guard must not release the current realization"),
            Err(error) => error,
        };
        assert!(error.contains("already has an active Source"));

        drop(third);
        let (fifth_input, _fifth_world) = UnixStream::pair().expect("fifth stdin stream");
        drop(
            binding
                .acquire(fifth_input)
                .expect("lease is reusable after the current owner releases it"),
        );
    }
}
