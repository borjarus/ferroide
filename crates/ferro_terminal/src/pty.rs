//! The PTY of a session and the thread that drives it.
//!
//! Each session has one I/O thread that owns `alacritty_terminal`'s
//! [`Pty`](tty::Pty): it reads the shell's output and feeds the session's
//! [`TerminalCore`](crate::TerminalCore), writes input and the emulator's
//! replies back, resizes, and notices the child exiting. Other threads
//! talk to it over a channel plus a wake-up, so the UI never blocks on the
//! PTY.
//!
//! The thread waits differently per platform, without `unsafe` (which
//! `polling`'s registration needs on Unix):
//!
//! - **Windows**: ConPTY's pipes are read and written by alacritty's helper
//!   threads, which post to a [`polling::Poller`]; its safe
//!   `reregister` arms them, and the child-exit watcher posts to the same
//!   poller. The pseudoconsole is closed (ending the shell) when the thread
//!   drops the `Pty`.
//! - **Unix**: the master side of the PTY is non-blocking; the thread
//!   `poll(2)`s it together with a socket that wakes it for input, with a
//!   short timeout to look for the child's exit (alacritty reports it
//!   through a SIGCHLD pipe that only `next_child_event` reads). Resizing
//!   uses `tcsetwinsize` directly, since alacritty's `on_resize` exits the
//!   process when the ioctl fails. Closing sends SIGHUP to the shell's
//!   process group, then SIGKILL if it lingers, so dropping the `Pty`
//!   (which waits for the child) cannot hang.
//!
//! When the shell exits, the thread reads what is left, reports the exit
//! and ends; the session keeps showing its output.

use std::{
    collections::HashMap,
    panic::AssertUnwindSafe,
    path::Path,
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use alacritty_terminal::{event::WindowSize, tty};
use anyhow::Context as _;

use crate::{service::SessionShared, shell::Launch};

/// A request for the I/O thread.
pub(crate) enum Control {
    Write(Vec<u8>),
    Resize(WindowSize),
    Shutdown,
}

/// The other end of a session's I/O thread.
pub(crate) struct PtyHandle {
    tx: mpsc::Sender<Control>,
    waker: Waker,
    thread: Option<JoinHandle<()>>,
    done: mpsc::Receiver<()>,
}

impl PtyHandle {
    pub fn send(&self, control: Control) {
        if self.tx.send(control).is_ok() {
            self.waker.wake();
        }
    }

    /// Stop the thread (ending the shell) and wait for it a little; a
    /// thread stuck in the OS is left behind rather than freezing the
    /// caller.
    fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.send(Control::Shutdown);
        match self.done.recv_timeout(Duration::from_secs(5)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                tracing::warn!("terminal I/O thread did not stop; leaving it behind");
            }
        }
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Signals the end of the thread, also when it panics.
struct DoneGuard(mpsc::Sender<()>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// Start `launch` in `cwd` on a new PTY and its I/O thread.
pub(crate) fn spawn(
    launch: &Launch,
    cwd: &Path,
    size: WindowSize,
    shared: Arc<SessionShared>,
) -> anyhow::Result<PtyHandle> {
    let program = launch.program.to_string_lossy().into_owned();
    #[cfg(windows)]
    let program = if program.contains(' ') && !program.starts_with('"') {
        // ConPTY's command line is the program followed by the arguments.
        format!("\"{program}\"")
    } else {
        program
    };
    let options = tty::Options {
        shell: Some(tty::Shell::new(program, launch.args.clone())),
        working_directory: Some(cwd.to_path_buf()),
        drain_on_exit: false,
        env: launch.env.iter().cloned().collect::<HashMap<_, _>>(),
        #[cfg(windows)]
        escape_args: true,
    };
    let id = shared.id;
    // alacritty asserts on some OS failures; a failed shell must not take
    // the IDE down.
    let pty = std::panic::catch_unwind(AssertUnwindSafe(|| tty::new(&options, size, id)))
        .map_err(|_| anyhow::anyhow!("the pseudoterminal could not be created"))?
        .with_context(|| format!("starting {}", launch.program.display()))?;

    let (tx, rx) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let (waker, io) = imp::prepare(&pty)?;
    let thread = std::thread::Builder::new()
        .name(format!("terminal-{id}"))
        .spawn(move || {
            let _done = DoneGuard(done_tx);
            imp::run(pty, io, rx, shared);
        })
        .context("starting the terminal I/O thread")?;
    Ok(PtyHandle {
        tx,
        waker,
        thread: Some(thread),
        done,
    })
}

/// Input and replies not yet written to the PTY.
#[derive(Default)]
struct Outbox(Vec<u8>);

impl Outbox {
    /// Write as much as the PTY takes now.
    fn flush(&mut self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        while !self.0.is_empty() {
            match writer.write(&self.0) {
                Ok(0) => break,
                Ok(n) => {
                    self.0.drain(..n);
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
}

/// The time until `deadline`, at most `cap`.
fn timeout_until(deadline: Option<Instant>, cap: Option<Duration>) -> Option<Duration> {
    let until = deadline.map(|d| d.saturating_duration_since(Instant::now()));
    match (until, cap) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Size of one read from the PTY.
const READ_SIZE: usize = 64 * 1024;

/// How long to keep reading after the shell exited, for its last output.
const DRAIN_TIME: Duration = Duration::from_millis(300);

#[cfg(windows)]
mod imp {
    use std::{
        io::Read as _,
        sync::{Arc, mpsc},
        time::{Duration, Instant},
    };

    use alacritty_terminal::{
        event::OnResize as _,
        tty::{self, ChildEvent, EventedPty as _, EventedReadWrite as _},
    };
    use polling::{Event, Events, PollMode, Poller};

    use super::{Control, DRAIN_TIME, Outbox, READ_SIZE, timeout_until};
    use crate::service::SessionShared;

    #[derive(Clone)]
    pub(crate) struct Waker(Arc<Poller>);

    impl Waker {
        pub fn wake(&self) {
            let _ = self.0.notify();
        }
    }

    pub(crate) fn prepare(_pty: &tty::Pty) -> anyhow::Result<(Waker, Arc<Poller>)> {
        let poller = Arc::new(Poller::new()?);
        Ok((Waker(poller.clone()), poller))
    }

    /// Read whatever is available; returns the number of bytes.
    fn read_available(
        pty: &mut tty::Pty,
        buf: &mut [u8],
        shared: &SessionShared,
        out: &mut Outbox,
    ) -> usize {
        let mut total = 0;
        loop {
            // The reader never blocks: 0 is "nothing now".
            match pty.reader().read(buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total += n;
                    out.0.extend(shared.on_output(&buf[..n]));
                }
            }
        }
        total
    }

    pub(crate) fn run(
        mut pty: tty::Pty,
        poller: Arc<Poller>,
        rx: mpsc::Receiver<Control>,
        shared: Arc<SessionShared>,
    ) {
        let mut interest = Event::readable(0);
        if let Err(err) = pty.reregister(&poller, interest, PollMode::Level) {
            tracing::warn!("terminal {}: cannot watch the PTY: {err}", shared.id);
            shared.on_exit(None);
            return;
        }
        let mut events = Events::new();
        let mut buf = vec![0; READ_SIZE];
        let mut out = Outbox::default();
        'run: loop {
            events.clear();
            let timeout = timeout_until(shared.next_deadline(), None);
            if let Err(err) = poller.wait(&mut events, timeout)
                && err.kind() != std::io::ErrorKind::Interrupted
            {
                tracing::warn!("terminal {}: polling failed: {err}", shared.id);
                break;
            }
            loop {
                match rx.try_recv() {
                    Ok(Control::Write(bytes)) => out.0.extend(bytes),
                    Ok(Control::Resize(size)) => {
                        // ConPTY asserts success; never let that panic out.
                        let resized =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                pty.on_resize(size)
                            }));
                        if resized.is_err() {
                            tracing::warn!("terminal {}: resize failed", shared.id);
                        }
                    }
                    Ok(Control::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break 'run,
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            read_available(&mut pty, &mut buf, &shared, &mut out);
            if let Some(ChildEvent::Exited(status)) = pty.next_child_event() {
                // Output still on its way through ConPTY's pipe.
                let start = Instant::now();
                let mut quiet = Instant::now();
                while start.elapsed() < DRAIN_TIME && quiet.elapsed() < Duration::from_millis(60) {
                    if read_available(&mut pty, &mut buf, &shared, &mut out) > 0 {
                        quiet = Instant::now();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                shared.on_exit(status.and_then(|s| s.code()));
                break;
            }
            out.0.extend(shared.on_tick());
            if let Err(err) = out.flush(pty.writer()) {
                tracing::warn!("terminal {}: writing failed: {err}", shared.id);
            }
            let needs_write = !out.0.is_empty();
            if needs_write != interest.writable {
                interest.writable = needs_write;
                let _ = pty.reregister(&poller, interest, PollMode::Level);
            }
        }
        let _ = pty.deregister(&poller);
        // Closing the pseudoconsole ends the shell.
        drop(pty);
    }
}

#[cfg(unix)]
mod imp {
    use std::{
        io::{ErrorKind, Read as _, Write as _},
        os::unix::net::UnixStream,
        sync::{Arc, mpsc},
        time::{Duration, Instant},
    };

    use alacritty_terminal::tty::{self, ChildEvent, EventedPty as _, EventedReadWrite as _};
    use rustix::{
        event::{PollFd, PollFlags, Timespec, poll},
        process::{Pid, Signal, kill_process, kill_process_group},
        termios::{Winsize, tcsetwinsize},
    };

    use super::{Control, DRAIN_TIME, Outbox, READ_SIZE, timeout_until};
    use crate::service::SessionShared;

    /// How often to look for the child's exit while nothing happens.
    const CHILD_CHECK: Duration = Duration::from_millis(200);

    pub(crate) struct Waker(UnixStream);

    impl Waker {
        pub fn wake(&self) {
            // Non-blocking: a full socket already means "wake up".
            let _ = (&self.0).write(&[1]);
        }
    }

    pub(crate) fn prepare(_pty: &tty::Pty) -> anyhow::Result<(Waker, UnixStream)> {
        let (tx, rx) = UnixStream::pair()?;
        tx.set_nonblocking(true)?;
        rx.set_nonblocking(true)?;
        Ok((Waker(tx), rx))
    }

    enum ReadEnd {
        /// Nothing more for now.
        Idle,
        /// The other side hung up (the shell and everything it started
        /// closed the terminal).
        HungUp,
    }

    fn read_available(
        pty: &mut tty::Pty,
        buf: &mut [u8],
        shared: &SessionShared,
        out: &mut Outbox,
    ) -> ReadEnd {
        loop {
            match pty.reader().read(buf) {
                Ok(0) => return ReadEnd::HungUp,
                Ok(n) => out.0.extend(shared.on_output(&buf[..n])),
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => {}
                    ErrorKind::WouldBlock => return ReadEnd::Idle,
                    // EIO on Linux once the child side is closed.
                    _ => return ReadEnd::HungUp,
                },
            }
        }
    }

    fn exit_code(status: Option<std::process::ExitStatus>) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt as _;
        let status = status?;
        status.code().or_else(|| status.signal().map(|s| 128 + s))
    }

    fn resize(pty: &tty::Pty, size: alacritty_terminal::event::WindowSize) {
        let winsize = Winsize {
            ws_row: size.num_lines,
            ws_col: size.num_cols,
            ws_xpixel: size.num_cols.saturating_mul(size.cell_width),
            ws_ypixel: size.num_lines.saturating_mul(size.cell_height),
        };
        if let Err(err) = tcsetwinsize(pty.file(), winsize) {
            tracing::debug!("resizing the PTY failed: {err}");
        }
    }

    /// End the shell: SIGHUP to its process group, SIGKILL if it lingers.
    fn terminate(pty: &mut tty::Pty) {
        let Some(pid) = Pid::from_raw(pty.child().id() as i32) else {
            return;
        };
        let _ = kill_process_group(pid, Signal::HUP);
        let _ = kill_process(pid, Signal::HUP);
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if pty.next_child_event().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = kill_process_group(pid, Signal::KILL);
        let _ = kill_process(pid, Signal::KILL);
    }

    pub(crate) fn run(
        mut pty: tty::Pty,
        wake: UnixStream,
        rx: mpsc::Receiver<Control>,
        shared: Arc<SessionShared>,
    ) {
        let mut buf = vec![0; READ_SIZE];
        let mut out = Outbox::default();
        let mut hung_up = false;
        let mut hung_up_at = None;
        loop {
            let timeout =
                timeout_until(shared.next_deadline(), Some(CHILD_CHECK)).unwrap_or(CHILD_CHECK);
            let timespec = Timespec {
                tv_sec: timeout.as_secs() as _,
                tv_nsec: timeout.subsec_nanos() as _,
            };
            {
                let mut flags = PollFlags::empty();
                if !hung_up {
                    flags |= PollFlags::IN;
                    if !out.0.is_empty() {
                        flags |= PollFlags::OUT;
                    }
                }
                let mut fds = [
                    PollFd::new(&wake, PollFlags::IN),
                    PollFd::new(pty.file(), flags),
                ];
                let polled = if hung_up {
                    poll(&mut fds[..1], Some(&timespec))
                } else {
                    poll(&mut fds, Some(&timespec))
                };
                if let Err(err) = polled
                    && err != rustix::io::Errno::INTR
                {
                    tracing::warn!("terminal {}: polling failed: {err}", shared.id);
                    break;
                }
            }
            // Empty the wake-up socket.
            let mut sink = [0u8; 64];
            while matches!((&wake).read(&mut sink), Ok(n) if n > 0) {}

            let mut shutdown = false;
            loop {
                match rx.try_recv() {
                    Ok(Control::Write(bytes)) => {
                        if !hung_up {
                            out.0.extend(bytes);
                        }
                    }
                    Ok(Control::Resize(size)) => resize(&pty, size),
                    Ok(Control::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                        shutdown = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if shutdown {
                // Still running: the loop ends when the shell exits.
                terminate(&mut pty);
                break;
            }
            if !hung_up
                && let ReadEnd::HungUp = read_available(&mut pty, &mut buf, &shared, &mut out)
            {
                hung_up = true;
                hung_up_at = Some(Instant::now());
                out.0.clear();
            }
            if let Some(ChildEvent::Exited(status)) = pty.next_child_event() {
                if !hung_up {
                    // What the shell wrote last.
                    let start = Instant::now();
                    while start.elapsed() < DRAIN_TIME {
                        if let ReadEnd::HungUp =
                            read_available(&mut pty, &mut buf, &shared, &mut out)
                        {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                shared.on_exit(exit_code(status));
                break;
            }
            // Hung up but the child lives on (it closed the terminal):
            // report the exit as unknown after a while.
            if hung_up_at.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
                terminate(&mut pty);
                shared.on_exit(None);
                break;
            }
            out.0.extend(shared.on_tick());
            if !hung_up && let Err(err) = out.flush(pty.writer()) {
                tracing::warn!("terminal {}: writing failed: {err}", shared.id);
            }
        }
        // Dropping the PTY hangs up and reaps the child (already gone).
        drop(pty);
    }
}

pub(crate) use imp::Waker;
