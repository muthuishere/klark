//! Kill-on-close Job Objects for `klark feed` commands (ADR 0015).
//!
//! A feed command starts suspended, joins its own job and only then runs, so
//! every process it starts is in the job too. Closing the job (a stop, or the
//! worker dying, which closes all its handles) ends the whole tree: the
//! Windows counterpart of the Unix process group.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, DETACHED_PROCESS,
            PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        },
    },
};

/// One kill-on-close job. Dropping it ends every process still in it.
pub(crate) struct FeedJob {
    job: usize,
}

impl FeedJob {
    pub(crate) fn new() -> std::io::Result<Self> {
        // SAFETY: no name, default security; the handle is owned by `Self`.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let this = Self { job: job as usize };
        // SAFETY: zeroed is a valid value of this plain C struct.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(|_| std::io::Error::other("job limits size exceeds u32"))?;
        // SAFETY: `limits` lives across the call and `size` is its size.
        let ok = unsafe {
            SetInformationJobObject(
                this.handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(this)
    }

    fn handle(&self) -> HANDLE {
        self.job as HANDLE
    }

    /// Puts a (suspended) child in the job, then lets it run.
    pub(crate) fn assign(&self, child: &std::process::Child) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: both handles are valid for the call; the child is ours.
        if unsafe { AssignProcessToJobObject(self.handle(), child.as_raw_handle().cast()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        super::resume_suspended_process(Some(child.id()))
    }
}

impl Drop for FeedJob {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once; KILL_ON_JOB_CLOSE ends
        // every process still in the job.
        unsafe { CloseHandle(self.handle()) };
    }
}

/// The jobs of the commands this worker runs, by the child's pid.
fn jobs() -> &'static Mutex<HashMap<u32, FeedJob>> {
    static JOBS: OnceLock<Mutex<HashMap<u32, FeedJob>>> = OnceLock::new();
    JOBS.get_or_init(Mutex::default)
}

/// Before spawn: start the command suspended, with no console window.
pub(crate) fn prepare_feed_command(c: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

/// After spawn: put the command in its own job and let it run. On failure
/// the child is killed, since it would otherwise stay suspended.
pub(crate) fn attach_feed_job(child: &mut std::process::Child) -> std::io::Result<()> {
    let job = FeedJob::new().and_then(|j| j.assign(child).map(|()| j));
    match job {
        Ok(job) => {
            if let Ok(mut map) = jobs().lock() {
                map.insert(child.id(), job);
            }
            Ok(())
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(e)
        }
    }
}

/// Ends the command's whole tree by closing its job.
pub(crate) fn close_feed_job(pid: u32) {
    let job = jobs().lock().ok().and_then(|mut map| map.remove(&pid));
    drop(job);
}

/// Starts a feed worker (or `feed resume`) with no console, in its own
/// process group, outside the caller's job when the job allows it, so closing
/// the pane or terminal that ran `klark feed add` does not end the feed (the
/// Unix `process_group(0)`). A job that forbids breakaway is not an error:
/// the worker then starts inside it.
pub(crate) fn spawn_detached(c: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
    c.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB);
    match c.spawn() {
        Ok(child) => Ok(child),
        // ERROR_ACCESS_DENIED: the caller's job does not allow breakaway.
        Err(e) if e.raw_os_error() == Some(5) => {
            c.creation_flags(base);
            c.spawn()
        }
        Err(e) => Err(e),
    }
}

/// Ends a feed worker by pid and waits up to `wait` for it to go. The
/// caller has checked (by the feed's run lock) that the pid is that feed's
/// worker. The worker's handles close with it, so its commands' kill-on-close
/// jobs end their trees too. Returns false when the process could not be
/// opened (already gone, or not ours).
pub(crate) fn terminate_worker(pid: u32, wait: std::time::Duration) -> bool {
    // SAFETY: plain handle calls; the handle is closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
        WaitForSingleObject(h, ms);
        CloseHandle(h);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn exits_within(child: &mut std::process::Child, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Closing the job ends the shell and the process it started.
    #[test]
    fn closing_the_feed_job_kills_the_tree() {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        let mut c = Command::new(comspec);
        c.args(["/C", "ping -n 60 127.0.0.1 >NUL"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        prepare_feed_command(&mut c);
        let mut child = c.spawn().expect("spawn cmd");
        attach_feed_job(&mut child).expect("attach the job");
        assert!(
            !exits_within(&mut child, Duration::from_millis(500)),
            "the command runs once resumed"
        );
        close_feed_job(child.id());
        assert!(
            exits_within(&mut child, Duration::from_secs(10)),
            "closing the job ends the command"
        );
    }
}
