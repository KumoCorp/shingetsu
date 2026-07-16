//! Safe one-shot child-process execution for Lua.
//!
//! A hand-rolled read/write loop over `io.popen` can self-deadlock
//! when a child fills its output pipe while the caller is still
//! feeding its input.  `process.run` exists to avoid that: it spawns a
//! command, drains standard output and error while writing standard
//! input, and returns once the child exits.  Streaming one direction
//! incrementally remains the job of `io.popen`.
//!
//! Spawning is gated behind [`crate::Libraries::EXEC`], the same
//! option that enables `io.popen` and `os.execute`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use shingetsu_vm::error::portable_io_error_description;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::value::Value;
use crate::{Bytes, VmError};

/// The command to run: either an argument vector spawned directly, or
/// a single string run through `/bin/sh -c`.
///
/// A Lua string is handed to the shell (matching the `io.popen` /
/// `os.execute` convention); a table is read as an argument vector,
/// so no quoting or metacharacter surprises apply.  Every element is
/// an `OsString`, whose `FromLua` accepts only a Lua string, so a
/// table of numbers or booleans is rejected element by element.  The
/// shell form requires a POSIX `/bin/sh` and is unsupported on
/// Windows.
#[derive(crate::FromLua, crate::LuaTyped)]
enum Cmd {
    /// A shell command line run via `/bin/sh -c`.
    Shell(OsString),
    /// A direct argument vector; the first element is the program.
    Argv(Vec<OsString>),
}

/// The `process.run` argument table.
#[derive(crate::FromLua, crate::LuaTyped)]
struct RunSpec {
    /// Program to run: an argument vector (spawned directly) or a
    /// string (run via `/bin/sh -c`).
    cmd: Cmd,
    /// Bytes to write to the child's standard input, then close it.
    /// Absent connects standard input to `/dev/null`.
    stdin: Option<Bytes>,
    /// Environment variables for the child, merged onto the inherited
    /// environment unless `clear_env` is set.
    env: Option<HashMap<OsString, OsString>>,
    /// Start from an empty environment instead of the parent's,
    /// before applying `env`.
    #[lua(default = false)]
    clear_env: bool,
    /// Working directory for the child.  Absent inherits the
    /// parent's.
    cwd: Option<OsString>,
    /// Wall-clock limit in seconds, after which the child's process
    /// group is killed.  Absent, non-finite, or non-positive means no
    /// limit.
    timeout: Option<f64>,
    /// Byte cap per captured stream.  Zero caps at zero bytes; absent
    /// or negative means no limit.
    max_output: Option<i64>,
    /// Seconds to wait for a killed child (and its process group) to be
    /// reaped before giving up and raising.  Bounds the final wait so a
    /// SIGKILL that does not take effect promptly cannot make the call
    /// block indefinitely.  Absent, non-finite, or non-positive uses
    /// the default of five seconds.
    reap_timeout: Option<f64>,
}

/// How long to wait for a killed child to be reaped before raising,
/// when `spec.reap_timeout` does not specify one.
const DEFAULT_REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// The table returned by `process.run`.
#[derive(crate::IntoLua, crate::LuaTyped)]
struct RunResult {
    /// Exit code, or `nil` when the child was terminated by a signal.
    code: Option<i64>,
    /// Terminating signal number (Unix only), or `nil` when the child
    /// exited normally.  Reports `SIGKILL` when the child was killed
    /// for `timeout`, `max_output`, or an I/O failure; check
    /// `timed_out`, `truncated`, and `io_error` to tell that from a
    /// signal the child received on its own.
    signal: Option<i64>,
    /// `true` when the child exited with code `0` and was not
    /// signalled.
    ok: bool,
    /// Captured standard output.
    stdout: Bytes,
    /// Captured standard error.
    stderr: Bytes,
    /// `true` when the child was killed for exceeding `timeout`.
    timed_out: bool,
    /// `true` when either captured stream reached `max_output`.
    /// Reaching the cap on one stream kills the child, so the other
    /// stream can also end up shorter than its own bytes would allow.
    truncated: bool,
    /// The first genuine I/O failure while reading a captured stream or
    /// writing standard input, prefixed with the stream name, or `nil`
    /// when none occurred.  A capture reflected here is incomplete;
    /// `ok` is `false` whenever it is set.  An ordinary broken pipe on
    /// standard input (a child like `head` exiting before reading all
    /// input) is not reported here.
    io_error: Option<Bytes>,
}

/// Build a Lua-side runtime error whose value is `msg`.
fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::string(msg),
    }
}

/// Split an [`ExitStatus`] into `(code, signal)`, where exactly one
/// side is `Some`: a signal number on a signalled Unix child,
/// otherwise the numeric exit code.
fn status_parts(status: ExitStatus) -> (Option<i64>, Option<i64>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return (None, Some(signal as i64));
        }
    }
    // A signalled Unix child returned above; every supported platform
    // reports a code here, so `-1` only stands in for a hypothetical
    // platform that offers neither.
    (Some(status.code().unwrap_or(-1) as i64), None)
}

/// Append `data` to `buf`, honouring an optional byte cap.  Sets
/// `truncated` and stops appending once the cap is reached.
fn append_capped(buf: &mut Vec<u8>, data: &[u8], cap: Option<usize>, truncated: &mut bool) {
    match cap {
        Some(cap) => {
            if buf.len() >= cap {
                *truncated = true;
                return;
            }
            let room = cap - buf.len();
            if data.len() > room {
                buf.extend_from_slice(&data[..room]);
                *truncated = true;
            } else {
                buf.extend_from_slice(data);
            }
        }
        None => buf.extend_from_slice(data),
    }
}

/// Build the [`Command`] for `spec`.  Standard input is piped only
/// when `spec.stdin` is provided, otherwise connected to `/dev/null`;
/// both outputs are always piped for capture.
fn build_command(spec: &RunSpec) -> Result<Command, VmError> {
    let mut cmd = match &spec.cmd {
        Cmd::Shell(line) => {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(line);
            c
        }
        Cmd::Argv(argv) => {
            let Some((prog, args)) = argv.split_first() else {
                return Err(VmError::BadArgument {
                    position: 1,
                    function: "process.run".to_owned(),
                    expected: "a non-empty command vector".to_owned(),
                    got: "an empty table".to_owned(),
                });
            };
            let mut c = Command::new(prog);
            c.args(args);
            c
        }
    };

    if spec.clear_env {
        cmd.env_clear();
    }
    if let Some(env) = &spec.env {
        cmd.envs(env);
    }
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(if spec.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    // Run the child as its own process-group leader so a timeout or
    // output cap can signal the whole group, reaching grandchildren a
    // shell pipeline spawned rather than only the direct child.
    #[cfg(unix)]
    cmd.process_group(0);

    Ok(cmd)
}

/// Accumulated output of a `communicate` pass.
#[derive(Default)]
struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
    /// The first I/O failure worth reporting; later ones are dropped
    /// because the capture is already known to be incomplete.
    io_error: Option<String>,
}

impl Captured {
    /// Record `msg` as the I/O failure unless one is already noted.
    fn note_io_error(&mut self, msg: impl FnOnce() -> String) {
        if self.io_error.is_none() {
            self.io_error = Some(msg());
        }
    }
}

/// Read from `stream` when present, or wait forever when it is `None`.
///
/// The `None` arm is never reached in [`communicate`], where every
/// call is paired with an `if stream.is_some()` select guard; it
/// exists only to give the disabled branch a future without an
/// `unwrap`.
async fn read_when_present(
    stream: &mut Option<impl tokio::io::AsyncRead + Unpin>,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    match stream.as_mut() {
        Some(stream) => stream.read(buf).await,
        None => std::future::pending().await,
    }
}

/// Write to `stdin` when present, or wait forever when it is `None`,
/// mirroring [`read_when_present`] for the input side.
async fn write_when_present(stdin: &mut Option<ChildStdin>, buf: &[u8]) -> std::io::Result<usize> {
    match stdin.as_mut() {
        Some(stdin) => stdin.write(buf).await,
        None => std::future::pending().await,
    }
}

/// Concurrently write `input` to the child's standard input and drain
/// its standard output and error into `captured`, returning when both
/// output streams reach end-of-file, or early once either exceeds
/// `cap`.
///
/// The write and both reads are interleaved to avoid deadlock when a
/// pipe buffer fills: a child that fills its output pipe while we are
/// still feeding its input never wedges, because a read is always
/// ready to run.  A read or write error ends that stream, keeping the
/// bytes gathered so far, and is recorded in `captured.io_error`; an
/// ordinary broken pipe on standard input (a child like `head`
/// exiting before reading all input) is expected and not recorded.
///
/// Because `captured` is borrowed from the caller, partial output
/// survives even if this future is dropped (for example when a
/// `timeout` cancels it).
///
/// Reaching the `cap` breaks out of the loop with the child still
/// running; unwedging it is left to the caller, which kills the child
/// after this returns.
///
/// `select!` drops the losing stdin write future each iteration, but
/// no input is duplicated: `ChildStdin::write` performs its `write(2)`
/// within one poll and buffers nothing, so a dropped write either
/// already advanced `written` or never issued the syscall.
///
/// One case still hangs absent a `timeout`: a child that closes both
/// output streams, stays alive, and never reads an `input` larger
/// than a pipe buffer leaves the write pending with nothing else to
/// drive.  Supply a `timeout` when feeding untrusted commands large
/// input.
async fn communicate(child: &mut Child, input: &[u8], cap: Option<usize>, captured: &mut Captured) {
    let mut stdin: Option<ChildStdin> = child.stdin.take();
    let mut stdout: Option<ChildStdout> = child.stdout.take();
    let mut stderr: Option<ChildStderr> = child.stderr.take();

    // With no input to send, close the child's standard input up front
    // so a child that reads to end-of-file is not left waiting.
    let mut written = 0usize;
    if input.is_empty() {
        stdin = None;
    }

    let mut out_chunk = [0u8; 8192];
    let mut err_chunk = [0u8; 8192];

    while stdout.is_some() || stderr.is_some() || stdin.is_some() {
        tokio::select! {
            res = read_when_present(&mut stdout, &mut out_chunk), if stdout.is_some() => {
                match res {
                    Ok(0) => stdout = None,
                    Err(e) => {
                        captured.note_io_error(|| format!("stdout: {}", portable_io_error_description(&e)));
                        stdout = None;
                    }
                    Ok(n) => {
                        append_capped(&mut captured.stdout, &out_chunk[..n], cap, &mut captured.truncated);
                        if captured.truncated {
                            break;
                        }
                    }
                }
            }
            res = read_when_present(&mut stderr, &mut err_chunk), if stderr.is_some() => {
                match res {
                    Ok(0) => stderr = None,
                    Err(e) => {
                        captured.note_io_error(|| format!("stderr: {}", portable_io_error_description(&e)));
                        stderr = None;
                    }
                    Ok(n) => {
                        append_capped(&mut captured.stderr, &err_chunk[..n], cap, &mut captured.truncated);
                        if captured.truncated {
                            break;
                        }
                    }
                }
            }
            res = write_when_present(&mut stdin, &input[written..]), if stdin.is_some() => {
                match res {
                    // The slice is never empty here (the `written >=
                    // input.len()` arm closes stdin first), so a
                    // zero-length write means the child will accept no
                    // more input; stop feeding it.
                    Ok(0) => stdin = None,
                    Err(e) => {
                        // A broken pipe is the ordinary case of a child
                        // that exits before reading all input; only a
                        // different failure is worth reporting.
                        if e.kind() != std::io::ErrorKind::BrokenPipe {
                            captured.note_io_error(|| format!("stdin: {}", portable_io_error_description(&e)));
                        }
                        stdin = None;
                    }
                    Ok(n) => {
                        written += n;
                        if written >= input.len() {
                            // Close the pipe to signal end-of-file.
                            stdin = None;
                        }
                    }
                }
            }
        }
    }
}

/// Install the `process` module, gated with `io.popen` and
/// `os.execute` under [`crate::Libraries::EXEC`].
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = process_mod::build_module_table(env)?;
    env.set_global("process", Value::Table(table));
    env.register_module_type("process", process_mod::module_type());
    Ok(())
}

/// Child-process execution with captured I/O.
///
/// `process.run` spawns a command and collects all of its output in a
/// single call without the pipe-buffer deadlock a hand-rolled
/// `io.popen` loop can hit.
#[crate::module(name = "process")]
mod process_mod {
    use super::*;

    /// Run a command to completion and return its captured output and
    /// exit status.
    ///
    /// The command is given as `spec.cmd`, which is either an argument
    /// vector (a table, spawned directly with no shell) or a string
    /// (run via `/bin/sh -c`).  Standard output and error are always
    /// captured; `spec.stdin`, when given, is written to the child and
    /// then closed.  Prefer this over driving an `io.popen` handle by
    /// hand whenever you just need the output.
    ///
    /// Set `spec.timeout` when running untrusted commands: without it,
    /// a child that never reads a `spec.stdin` larger than a pipe
    /// buffer does not return.
    ///
    /// # Parameters
    ///
    /// - `spec` -- a table of options.  `cmd` is required (an argument
    ///   vector or a shell command string); `stdin`, `env`,
    ///   `clear_env`, `cwd`, `timeout`, `max_output`, and
    ///   `reap_timeout` are optional and documented on their fields.
    ///
    /// # Returns
    ///
    /// A table with fields `code`, `signal`, `ok`, `stdout`, `stderr`,
    /// `timed_out`, `truncated`, and `io_error`.  `code` is `nil` when
    /// the child was signalled; `signal` is `nil` when it exited
    /// normally; `io_error` is `nil` unless a stream failed to capture.
    ///
    /// Raises if the argument vector is empty, `spec.cmd` is neither a
    /// string nor a table, the child cannot be spawned (for example,
    /// the program does not exist), or a killed child cannot be reaped
    /// within `spec.reap_timeout`.
    ///
    /// # Examples
    ///
    /// ```lua,no_run
    /// -- Capture output without a shell.
    /// local r = process.run{ cmd = {"printf", "hello"} }
    /// assert(r.ok)
    /// assert(r.stdout == "hello")
    /// ```
    ///
    /// ```lua,no_run
    /// -- Feed standard input and read the transformed output back.
    /// local r = process.run{ cmd = {"tr", "a-z", "A-Z"}, stdin = "hi\n" }
    /// assert(r.stdout == "HI\n")
    /// ```
    ///
    /// ```lua,no_run
    /// -- Use shell features by passing a string.
    /// local r = process.run{ cmd = "printf hi | tr a-z A-Z" }
    /// assert(r.stdout == "HI")
    /// ```
    #[function]
    async fn run(spec: RunSpec) -> Result<RunResult, VmError> {
        run_impl(spec).await
    }
}

/// Kill the child.  On Unix this signals the whole process group
/// established by `process_group(0)`, reaching grandchildren a shell
/// pipeline spawned rather than only the direct child.
///
/// Must run before the child is reaped: it targets `child.id()`,
/// which the kernel may recycle for an unrelated process once the
/// child is waited on.  A failure to signal (for example `EPERM` on a
/// locked-down host) is ignored; the caller's bounded reap then
/// governs how long the still-live child is awaited.
fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // A negative pid targets the process group led by the child.
        // Safety: `kill` has no memory effects; an invalid pid (the
        // child already reaped) simply returns an error we ignore.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// Spawn, feed, drain, and wait for the process described by `spec`.
async fn run_impl(spec: RunSpec) -> Result<RunResult, VmError> {
    let mut command = build_command(&spec)?;
    let mut child = command.spawn().map_err(|e| {
        runtime_error(format!(
            "failed to spawn process: {}",
            portable_io_error_description(&e)
        ))
    })?;

    let cap = match spec.max_output {
        Some(n) if n >= 0 => Some(n as usize),
        _ => None,
    };
    let input = spec.stdin.unwrap_or_default();

    // `try_from_secs_f64` rejects non-finite, non-positive, and
    // overflowing values, any of which means run without a limit; a
    // zero duration would fire immediately, so drop it too.
    let deadline = spec
        .timeout
        .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
        .filter(|d| !d.is_zero());
    let reap = reap_timeout(spec.reap_timeout);

    let mut captured = Captured::default();
    let comm = communicate(&mut child, &input, cap, &mut captured);

    // A timeout cancels `comm`; the partial capture survives because
    // `captured` is borrowed from this scope (see `communicate`).
    let timed_out = match deadline {
        Some(d) => tokio::time::timeout(d, comm).await.is_err(),
        None => {
            comm.await;
            false
        }
    };

    let wait_error = |e: std::io::Error| {
        runtime_error(format!(
            "failed to wait for process: {}",
            portable_io_error_description(&e)
        ))
    };

    // A read error leaves the child possibly still running; kill and
    // bounded-reap it too, so a live child cannot block `wait()`
    // unbounded on the error path.
    let status = if timed_out || captured.truncated || captured.io_error.is_some() {
        kill_child(&mut child);
        // The group has been sent SIGKILL; bound the reap so a signal
        // that does not take effect promptly cannot block the call.
        match tokio::time::timeout(reap, child.wait()).await {
            Ok(status) => status.map_err(wait_error)?,
            Err(_) => {
                return Err(runtime_error(format!(
                    "killed process could not be reaped within {} seconds",
                    reap.as_secs_f64()
                )));
            }
        }
    } else {
        child.wait().await.map_err(wait_error)?
    };
    let (code, signal) = status_parts(status);

    Ok(RunResult {
        ok: code == Some(0)
            && signal.is_none()
            && !timed_out
            && !captured.truncated
            && captured.io_error.is_none(),
        code,
        signal,
        stdout: Bytes::from(captured.stdout),
        stderr: Bytes::from(captured.stderr),
        timed_out,
        truncated: captured.truncated,
        io_error: captured.io_error.map(Bytes::from),
    })
}

/// The reap duration named by `secs` when it is a positive finite
/// value, otherwise [`DEFAULT_REAP_TIMEOUT`].
fn reap_timeout(secs: Option<f64>) -> Duration {
    secs.and_then(|secs| Duration::try_from_secs_f64(secs).ok())
        .filter(|d| !d.is_zero())
        .unwrap_or(DEFAULT_REAP_TIMEOUT)
}
