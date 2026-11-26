//! One app instance per user session. A later launch asks the running instance
//! to open its board and exits instead of starting a second tray host.
//!
//! Windows decides by named-mutex ownership, so a mutex abandoned by a crashed
//! process is reclaimed at once, and signals through named activate and ack
//! events. Unix holds an `flock`, which the kernel drops when the process dies,
//! and binds its activation socket only while it holds that lock.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

/// A later launch waits this long for a quitting instance to release the guard.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(10);
/// An unanswered activation request is repeated at this interval.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const NO_RESPONSE: &str =
    "Token Ashtray is already running but did not respond. Try again in a moment.";

/// Names the guard and its activation channel. Tests use an isolated scope so
/// they never signal an installed instance.
#[derive(Clone, Debug)]
pub struct Scope {
    #[cfg(windows)]
    name: String,
    #[cfg(unix)]
    dir: std::path::PathBuf,
}

impl Scope {
    /// The installed app's scope: one instance per user session.
    pub fn user_session() -> Option<Self> {
        #[cfg(windows)]
        {
            // Release-stable, derived from the installer AppId, so the installer
            // and later releases can find the same object.
            Some(Self {
                name: r"Local\TokenAshtray-{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}".into(),
            })
        }
        #[cfg(unix)]
        {
            let dirs = crate::config::project_dirs()?;
            let dir = dirs.runtime_dir().unwrap_or(dirs.cache_dir());
            Some(Self { dir: dir.into() })
        }
    }

    /// A private scope that no installed instance uses.
    pub fn isolated(id: &str) -> Self {
        #[cfg(windows)]
        {
            Self {
                name: format!(r"Local\TokenAshtray-isolated-{id}"),
            }
        }
        #[cfg(unix)]
        {
            Self {
                dir: std::env::temp_dir().join(format!("token-ashtray-isolated-{id}")),
            }
        }
    }
}

pub enum Instance {
    /// This process is the only instance and holds the guard.
    Primary(Primary),
    /// Another instance holds the guard.
    Running(Running),
}

/// The guard and its activation listener. On Windows the guard belongs to the
/// acquiring thread, so `Primary` stays on that thread until the process ends.
pub struct Primary {
    _guard: Option<Guard>,
    listener: Option<Listener>,
}

impl Primary {
    /// Runs without single-instance protection when the guard cannot be created.
    pub fn unguarded() -> Self {
        Self {
            _guard: None,
            listener: None,
        }
    }

    pub fn listener(&self) -> Option<&Listener> {
        self.listener.as_ref()
    }

    pub fn take_listener(&mut self) -> Option<Listener> {
        self.listener.take()
    }
}

impl Listener {
    /// Starts a thread that forwards requests and calls `wake` after each one.
    /// The receiver acknowledges each request after it has acted on it.
    pub fn spawn(self, wake: impl Fn() + Send + 'static) -> mpsc::Receiver<Request> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            while let Some(mut request) = self.next() {
                // Take the next request only after this one is answered or
                // dropped, so one waiting launch gets one answer.
                let (answered, unanswered) = mpsc::channel::<()>();
                request._answered = Some(answered);
                if sender.send(request).is_err() {
                    break;
                }
                wake();
                let _ = unanswered.recv();
            }
        });
        receiver
    }
}

#[cfg(windows)]
mod sys {
    use super::*;
    use std::{
        io,
        marker::PhantomData,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr::null,
        sync::Arc,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, WAIT_ABANDONED, WAIT_ABANDONED_0, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{
            CreateEventW, CreateMutexW, INFINITE, OpenMutexW, ReleaseMutex, ResetEvent,
            SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
        },
        UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
    };

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(Some(0)).collect()
    }

    fn owned(handle: HANDLE, object: &str) -> Result<OwnedHandle, String> {
        if handle.is_null() {
            Err(format!(
                "could not open the instance {object}: {}",
                io::Error::last_os_error()
            ))
        } else {
            // The handle was just returned to this process and is closed once.
            Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
        }
    }

    fn event(name: &str) -> Result<OwnedHandle, String> {
        // Auto-reset and initially clear; create-or-open, so either side may be first.
        owned(
            unsafe { CreateEventW(null(), 0, 0, wide(name).as_ptr()) },
            "event",
        )
    }

    pub(super) struct Guard {
        mutex: OwnedHandle,
        // Mutex ownership belongs to the acquiring thread.
        _thread: PhantomData<*const ()>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe { ReleaseMutex(self.mutex.as_raw_handle()) };
        }
    }

    pub struct Listener {
        activate: OwnedHandle,
        ack: Arc<OwnedHandle>,
    }

    pub struct Request {
        ack: Arc<OwnedHandle>,
        pub(super) _answered: Option<mpsc::Sender<()>>,
    }

    pub struct Running {
        mutex: OwnedHandle,
        activate: OwnedHandle,
        ack: OwnedHandle,
    }

    /// Keeps the guard object alive without owning it.
    pub struct ProcessMark {
        _handle: Option<OwnedHandle>,
    }

    fn primary(mutex: OwnedHandle, activate: OwnedHandle, ack: OwnedHandle) -> Primary {
        Primary {
            _guard: Some(Guard {
                mutex,
                _thread: PhantomData,
            }),
            listener: Some(Listener {
                activate,
                ack: Arc::new(ack),
            }),
        }
    }

    pub fn acquire(scope: &Scope) -> Result<Instance, String> {
        let mutex = owned(
            unsafe { CreateMutexW(null(), 0, wide(&scope.name).as_ptr()) },
            "mutex",
        )?;
        // Open both events before checking ownership: a later launch can then
        // signal an instance that owns the mutex but has not reached its loop.
        let activate = event(&format!("{}-activate", scope.name))?;
        let ack = event(&format!("{}-ack", scope.name))?;
        // Decide by ownership, not existence: a crashed owner leaves an
        // abandoned mutex, and an orphaned UI child may still hold a handle.
        match unsafe { WaitForSingleObject(mutex.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Instance::Primary(primary(mutex, activate, ack))),
            WAIT_TIMEOUT => Ok(Instance::Running(Running {
                mutex,
                activate,
                ack,
            })),
            _ => Err(format!(
                "could not check the running instance: {}",
                io::Error::last_os_error()
            )),
        }
    }

    impl Scope {
        /// Holds the guard object open for this process without owning it, so
        /// the object exists until every app process has exited.
        pub fn mark_app_process(&self) -> ProcessMark {
            let handle =
                unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(&self.name).as_ptr()) };
            ProcessMark {
                _handle: owned(handle, "mutex").ok(),
            }
        }
    }

    impl Running {
        /// Asks the running instance to open its board. Returns the guard when
        /// that instance exits first, as after Quit, so this launch starts.
        pub fn activate(self) -> Result<Option<Primary>, String> {
            self.activate_with(|| {})
        }

        /// `between_resends` runs after each unanswered wait, before the
        /// request is sent again.
        pub(super) fn activate_with(
            self,
            between_resends: impl Fn(),
        ) -> Result<Option<Primary>, String> {
            let deadline = Instant::now() + HANDOFF_TIMEOUT;
            let handles = [self.ack.as_raw_handle(), self.mutex.as_raw_handle()];
            // This launch may take the foreground; let the running board take it.
            unsafe { AllowSetForegroundWindow(ASFW_ANY) };
            // Clear an answer left over for an earlier launch once. A later
            // reset could erase this launch's answer arriving between resends.
            unsafe { ResetEvent(self.ack.as_raw_handle()) };
            loop {
                unsafe { SetEvent(self.activate.as_raw_handle()) };
                let result = unsafe {
                    WaitForMultipleObjects(
                        2,
                        handles.as_ptr(),
                        0,
                        RETRY_INTERVAL.as_millis() as u32,
                    )
                };
                if result == WAIT_TIMEOUT && Instant::now() < deadline {
                    between_resends();
                    continue;
                }
                let error = io::Error::last_os_error();
                // Withdraw any resend the instance has not taken, so it does
                // not answer again after this launch has gone.
                unsafe { ResetEvent(self.activate.as_raw_handle()) };
                return match result {
                    WAIT_OBJECT_0 => Ok(None),
                    WAIT_TIMEOUT => Err(NO_RESPONSE.into()),
                    owner if owner == WAIT_OBJECT_0 + 1 || owner == WAIT_ABANDONED_0 + 1 => {
                        Ok(Some(primary(self.mutex, self.activate, self.ack)))
                    }
                    _ => Err(format!("could not signal the running instance: {error}")),
                };
            }
        }
    }

    impl Listener {
        fn wait(&self, milliseconds: u32) -> Option<Request> {
            (unsafe { WaitForSingleObject(self.activate.as_raw_handle(), milliseconds) }
                == WAIT_OBJECT_0)
                .then(|| Request {
                    ack: Arc::clone(&self.ack),
                    _answered: None,
                })
        }

        /// Returns a pending request without blocking.
        pub fn try_next(&self) -> Option<Request> {
            self.wait(0)
        }

        pub(super) fn next(&self) -> Option<Request> {
            self.wait(INFINITE)
        }
    }

    impl Request {
        /// Tells the later launch that this instance has acted, so it exits.
        pub fn acknowledge(self) {
            unsafe { SetEvent(self.ack.as_raw_handle()) };
        }
    }
}

#[cfg(unix)]
mod sys {
    use super::*;
    use std::{
        fs::{self, File, OpenOptions, TryLockError},
        io::{ErrorKind, Read, Write},
        os::unix::{
            fs::DirBuilderExt,
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
    };

    pub(super) struct Guard {
        _lock: File,
    }

    pub struct Listener {
        socket: UnixListener,
    }

    pub struct Request {
        stream: UnixStream,
        pub(super) _answered: Option<mpsc::Sender<()>>,
    }

    pub struct Running {
        lock: File,
        socket: PathBuf,
    }

    fn primary(lock: File, socket: &Path) -> Result<Primary, String> {
        // Only the lock holder replaces the socket, so a live instance's
        // socket is never removed.
        match fs::remove_file(socket) {
            Err(error) if error.kind() != ErrorKind::NotFound => {
                return Err(format!("could not replace {}: {error}", socket.display()));
            }
            _ => {}
        }
        let listener = UnixListener::bind(socket)
            .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            .map_err(|error| format!("could not listen on {}: {error}", socket.display()))?;
        Ok(Primary {
            _guard: Some(Guard { _lock: lock }),
            listener: Some(Listener { socket: listener }),
        })
    }

    pub fn acquire(scope: &Scope) -> Result<Instance, String> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&scope.dir)
            .map_err(|error| format!("could not create {}: {error}", scope.dir.display()))?;
        let path = scope.dir.join("instance.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        let socket = scope.dir.join("activate.sock");
        match lock.try_lock() {
            Ok(()) => primary(lock, &socket).map(Instance::Primary),
            Err(TryLockError::WouldBlock) => Ok(Instance::Running(Running { lock, socket })),
            Err(TryLockError::Error(error)) => {
                Err(format!("could not lock {}: {error}", path.display()))
            }
        }
    }

    impl Running {
        /// Asks the running instance to open its board. Returns the guard when
        /// that instance exits first, as after Quit, so this launch starts.
        pub fn activate(self) -> Result<Option<Primary>, String> {
            let deadline = Instant::now() + HANDOFF_TIMEOUT;
            // One queued connection is one request. Reconnect only after the
            // instance closes it, so a slow instance does not act twice.
            let mut request = None::<UnixStream>;
            loop {
                // The socket is missing while the instance starts or after it exits.
                if request.is_none() {
                    request = UnixStream::connect(&self.socket)
                        .ok()
                        .filter(|stream| stream.set_read_timeout(Some(RETRY_INTERVAL)).is_ok());
                }
                if let Some(stream) = request.as_mut() {
                    match stream.read(&mut [0]) {
                        Ok(1) => return Ok(None),
                        Err(error)
                            if matches!(
                                error.kind(),
                                ErrorKind::WouldBlock | ErrorKind::TimedOut
                            ) => {}
                        // Closed unanswered: the instance is quitting or has exited.
                        _ => request = None,
                    }
                }
                match self.lock.try_lock() {
                    Ok(()) => return primary(self.lock, &self.socket).map(Some),
                    Err(TryLockError::WouldBlock) => {}
                    Err(TryLockError::Error(error)) => {
                        return Err(format!("could not check the running instance: {error}"));
                    }
                }
                if Instant::now() >= deadline {
                    return Err(NO_RESPONSE.into());
                }
                if request.is_none() {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    impl Listener {
        /// Returns a pending request without blocking.
        pub fn try_next(&self) -> Option<Request> {
            let (stream, _) = self.socket.accept().ok()?;
            Some(Request {
                stream,
                _answered: None,
            })
        }

        pub(super) fn next(&self) -> Option<Request> {
            self.socket.set_nonblocking(false).ok()?;
            loop {
                match self.socket.accept() {
                    Ok((stream, _)) => {
                        return Some(Request {
                            stream,
                            _answered: None,
                        });
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    // Back off from persistent errors such as descriptor exhaustion.
                    Err(_) => std::thread::sleep(RETRY_INTERVAL),
                }
            }
        }
    }

    impl Request {
        /// Tells the later launch that this instance has acted, so it exits.
        pub fn acknowledge(mut self) {
            let _ = self.stream.set_nonblocking(false);
            let _ = self.stream.write_all(b"\n");
        }
    }
}

use sys::Guard;
#[cfg(windows)]
pub use sys::ProcessMark;
pub use sys::{Listener, Request, Running, acquire};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, ChildStdout, Command, Stdio},
        sync::atomic::{AtomicUsize, Ordering},
    };

    const CHILD_SCOPE: &str = "TOKEN_ASHTRAY_INSTANCE_TEST_SCOPE";
    const CHILD_MODE: &str = "TOKEN_ASHTRAY_INSTANCE_TEST_MODE";
    const READY: &str = "INSTANCE-TEST-READY";
    const ACTIVATED: &str = "INSTANCE-TEST-ACTIVATED";

    /// Unique among live processes and parallel tests; clock readings can repeat.
    fn unique_id() -> String {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Takes a guard that was just released. On Unix a process that a
    /// parallel test is spawning holds a copy of the lock descriptor until it
    /// executes, so the release can show up a moment late.
    fn reacquire(scope: &Scope) -> Primary {
        let deadline = Instant::now() + HANDOFF_TIMEOUT;
        loop {
            match acquire(scope) {
                Ok(Instance::Primary(primary)) => return primary,
                Ok(Instance::Running(_)) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(Instance::Running(_)) => panic!("a released guard must not stay held"),
                Err(error) => panic!("{error}"),
            }
        }
    }

    /// Plays the first instance, only when a test below re-executes this binary.
    #[test]
    fn instance_test_child() {
        let (Ok(id), Ok(mode)) = (std::env::var(CHILD_SCOPE), std::env::var(CHILD_MODE)) else {
            return;
        };
        let Ok(Instance::Primary(primary)) = acquire(&Scope::isolated(&id)) else {
            panic!("the child starts first and must hold the guard");
        };
        println!("{READY}");
        match mode.as_str() {
            // Quit: shut down for a while without answering, then exit.
            "quit" => std::thread::sleep(Duration::from_millis(1500)),
            serve => {
                if serve == "slow" {
                    std::thread::sleep(Duration::from_millis(1500));
                }
                let end = Instant::now() + Duration::from_secs(30);
                while Instant::now() < end {
                    while let Some(request) = primary.listener().and_then(Listener::try_next) {
                        // Record the open before answering: the parent may
                        // kill this process as soon as the launch is answered.
                        println!("{ACTIVATED}");
                        request.acknowledge();
                    }
                    // The host loop's poll interval.
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
        }
    }

    struct FirstInstance {
        child: Child,
        output: BufReader<ChildStdout>,
    }

    impl FirstInstance {
        fn start(id: &str, mode: &str) -> Self {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "platform::instance::tests::instance_test_child",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_SCOPE, id)
                .env(CHILD_MODE, mode)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            while !line.contains(READY) {
                line.clear();
                assert_ne!(
                    output.read_line(&mut line).unwrap(),
                    0,
                    "child exited early"
                );
            }
            Self { child, output }
        }

        /// Kills the first instance as a crash would; returns its activation count.
        fn crash(mut self) -> usize {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
            let mut rest = String::new();
            std::io::Read::read_to_string(&mut self.output, &mut rest).unwrap();
            rest.lines().filter(|line| line.contains(ACTIVATED)).count()
        }
    }

    impl Drop for FirstInstance {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn running(scope: &Scope) -> Running {
        match acquire(scope) {
            Ok(Instance::Running(running)) => running,
            Ok(Instance::Primary(_)) => panic!("a second launch must find the running instance"),
            Err(error) => panic!("{error}"),
        }
    }

    #[test]
    fn later_launch_activates_running_instance_once_and_crash_frees_guard() {
        let id = unique_id();
        let scope = Scope::isolated(&id);
        let first = FirstInstance::start(&id, "serve");
        assert!(
            running(&scope).activate().unwrap().is_none(),
            "the running instance must accept the request"
        );
        assert_eq!(
            first.crash(),
            1,
            "the running instance opens the board once"
        );
        assert!(
            matches!(acquire(&scope), Ok(Instance::Primary(_))),
            "a crashed instance must not leave a stale guard"
        );
    }

    #[test]
    fn launch_during_quit_starts_after_running_instance_exits() {
        let id = unique_id();
        let scope = Scope::isolated(&id);
        let _first = FirstInstance::start(&id, "quit");
        let started = Instant::now();
        let primary = running(&scope).activate().unwrap();
        assert!(
            primary.is_some_and(|primary| primary.listener().is_some()),
            "a relaunch during Quit must start, not be swallowed"
        );
        assert!(started.elapsed() < HANDOFF_TIMEOUT);
    }

    #[test]
    fn slow_running_instance_still_receives_the_request() {
        let id = unique_id();
        let scope = Scope::isolated(&id);
        let first = FirstInstance::start(&id, "slow");
        assert!(running(&scope).activate().unwrap().is_none());
        assert_eq!(first.crash(), 1);
    }

    #[test]
    fn late_acknowledgement_does_not_swallow_a_relaunch_during_quit() {
        let scope = Scope::isolated(&unique_id());
        let Ok(Instance::Primary(mut primary)) = acquire(&scope) else {
            panic!("an unused scope starts as primary");
        };
        let requests = primary.take_listener().unwrap().spawn(|| {});
        let launch = || {
            let scope = scope.clone();
            std::thread::spawn(move || running(&scope).activate().map(|own| own.is_none()))
        };
        let first = launch();
        // A busy standalone board answers only after the launch has resent its request.
        std::thread::sleep(RETRY_INTERVAL * 5 / 2);
        while !first.is_finished() {
            if let Ok(request) = requests.recv_timeout(Duration::from_millis(50)) {
                request.acknowledge();
            }
        }
        assert!(first.join().unwrap().unwrap(), "the first launch hands off");
        // Quit: the board stops answering, then releases the guard.
        let relaunch = launch();
        std::thread::sleep(Duration::from_millis(1500));
        drop(requests);
        drop(primary);
        assert!(
            !relaunch.join().unwrap().unwrap(),
            "a relaunch during Quit must start, not be swallowed by an earlier answer"
        );
    }

    #[cfg(windows)]
    #[test]
    fn answer_between_resends_ends_the_launch_after_one_open() {
        let scope = Scope::isolated(&unique_id());
        let Ok(Instance::Primary(mut primary)) = acquire(&scope) else {
            panic!("an unused scope starts as primary");
        };
        let listener = primary.take_listener().unwrap();
        let (opened, opens) = mpsc::channel();
        let other = scope.clone();
        let launch = std::thread::spawn(move || {
            // The busy instance takes the request and answers just after the
            // launch stopped waiting and before it asks again.
            running(&other)
                .activate_with(|| {
                    if let Some(request) = listener.try_next() {
                        opened.send(()).unwrap();
                        request.acknowledge();
                    }
                })
                .map(|own| own.is_none())
        });
        assert_eq!(
            launch.join().unwrap(),
            Ok(true),
            "an answer that arrives between resends must end the launch"
        );
        assert_eq!(
            opens.try_iter().count(),
            1,
            "the instance opens the board once"
        );
    }

    #[test]
    fn simultaneous_launches_both_hand_off() {
        let id = unique_id();
        let scope = Scope::isolated(&id);
        let first = FirstInstance::start(&id, "serve");
        let launches = [running(&scope), running(&scope)]
            .map(|launch| std::thread::spawn(move || launch.activate().map(|own| own.is_none())));
        for launch in launches {
            assert!(launch.join().unwrap().unwrap(), "each launch must hand off");
        }
        assert!(first.crash() >= 1);
    }

    #[test]
    fn guard_belongs_to_one_holder_until_dropped() {
        let scope = Scope::isolated(&unique_id());
        let Ok(Instance::Primary(primary)) = acquire(&scope) else {
            panic!("an unused scope starts as primary");
        };
        // Windows mutex ownership is per thread: ask from another one.
        let other = scope.clone();
        let busy = std::thread::spawn(move || matches!(acquire(&other), Ok(Instance::Running(_))));
        assert!(
            busy.join().unwrap(),
            "a held guard must report a running instance"
        );
        drop(primary);
        // Quit must release the guard.
        std::thread::spawn(move || {
            reacquire(&scope);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn spawned_listener_wakes_and_waits_for_acknowledgement() {
        let scope = Scope::isolated(&unique_id());
        let Ok(Instance::Primary(mut primary)) = acquire(&scope) else {
            panic!("an unused scope starts as primary");
        };
        let (woken, wakes) = mpsc::channel();
        let requests = primary
            .take_listener()
            .unwrap()
            .spawn(move || woken.send(()).unwrap());
        let other = scope.clone();
        let launch =
            std::thread::spawn(move || running(&other).activate().map(|own| own.is_none()));
        wakes.recv_timeout(HANDOFF_TIMEOUT).unwrap();
        requests
            .recv_timeout(HANDOFF_TIMEOUT)
            .unwrap()
            .acknowledge();
        assert!(launch.join().unwrap().unwrap());
    }
}
