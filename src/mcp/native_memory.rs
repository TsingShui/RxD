//! Sample RSS only for the owned Native worker on the async supervisor.
use crate::query::QueryError;
use std::time::Duration;
use tokio::process::Child;

pub struct Monitor {
    process: platform::OwnedProcess,
    limit: u64,
}

impl Monitor {
    pub fn new(child: &Child, limit: usize) -> Result<Self, QueryError> {
        if limit == 0 {
            return Err(QueryError::new(
                "INVALID_QUERY",
                "native worker RSS limit must be positive",
            ));
        }
        Ok(Self {
            process: platform::OwnedProcess::new(child)?,
            limit: limit as u64,
        })
    }

    pub async fn exceeded(&self) -> QueryError {
        let mut interval = tokio::time::interval(Duration::from_millis(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match self.process.resident_bytes() {
                Ok(Some(bytes)) if bytes > self.limit => {
                    return QueryError::new(
                        "RESOURCE_LIMIT",
                        format!(
                            "native worker RSS {bytes} bytes exceeded {} byte limit",
                            self.limit
                        ),
                    );
                }
                Ok(Some(_)) => {}
                // The owned process exited/reused its PID. Its IPC/wait future remains
                // authoritative; never sample or terminate a replacement process.
                Ok(None) => return std::future::pending().await,
                Err(error) => return error,
            }
        }
    }
}

fn query_error(error: std::io::Error) -> QueryError {
    QueryError::new(
        "WORKER_ERROR",
        format!("cannot enforce native worker RSS limit: {error}"),
    )
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod platform {
    use super::*;
    use std::io::Read;

    struct Sample {
        parent: u32,
        birth: u64,
        pages: u64,
        zombie: bool,
    }
    pub struct OwnedProcess {
        pid: u32,
        birth: Option<u64>,
        page_bytes: u64,
    }
    impl OwnedProcess {
        pub fn new(child: &Child) -> Result<Self, QueryError> {
            let pid = child
                .id()
                .ok_or_else(|| QueryError::new("WORKER_ERROR", "native worker PID unavailable"))?;
            // SAFETY: sysconf has no pointer arguments and reports the kernel's page size.
            let page_bytes = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if page_bytes <= 0 {
                return Err(query_error(std::io::Error::last_os_error()));
            }
            let sample = read(pid)?;
            let birth = sample
                .filter(|sample| sample.parent == std::process::id() && !sample.zombie)
                .map(|sample| sample.birth);
            Ok(Self {
                pid,
                birth,
                page_bytes: page_bytes as u64,
            })
        }
        pub fn resident_bytes(&self) -> Result<Option<u64>, QueryError> {
            let Some(birth) = self.birth else {
                return Ok(None);
            };
            let Some(sample) = read(self.pid)? else {
                return Ok(None);
            };
            if sample.parent != std::process::id() || sample.birth != birth || sample.zombie {
                return Ok(None);
            }
            sample
                .pages
                .checked_mul(self.page_bytes)
                .map(Some)
                .ok_or_else(|| QueryError::new("WORKER_ERROR", "native RSS value overflows"))
        }
    }
    fn read(pid: u32) -> Result<Option<Sample>, QueryError> {
        let file = match std::fs::File::open(format!("/proc/{pid}/stat")) {
            Ok(file) => file,
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => {
                return Ok(None);
            }
            Err(error) => return Err(query_error(error)),
        };
        let mut text = String::new();
        file.take(4097)
            .read_to_string(&mut text)
            .map_err(query_error)?;
        let bad = || QueryError::new("WORKER_ERROR", "invalid owned-worker /proc statistics");
        if text.len() > 4096 {
            return Err(bad());
        }
        let suffix = text.rsplit_once(')').ok_or_else(bad)?.1;
        let fields: Vec<&str> = suffix.split_whitespace().collect();
        let parse = |index: usize| {
            fields
                .get(index)
                .ok_or_else(bad)?
                .parse::<u64>()
                .map_err(|_| bad())
        };
        let parent = u32::try_from(parse(1)?).map_err(|_| bad())?;
        Ok(Some(Sample {
            parent,
            birth: parse(19)?,
            pages: parse(21)?,
            zombie: fields.first() == Some(&"Z"),
        }))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    // Layouts and selectors verified against the calling host's macOS SDK
    // libproc.h and sys/proc_info.h, not private analyzed program bytes.
    #[repr(C)]
    struct BsdInfo {
        flags: u32,
        status: u32,
        xstatus: u32,
        pid: u32,
        parent: u32,
        uid: u32,
        gid: u32,
        ruid: u32,
        rgid: u32,
        svuid: u32,
        svgid: u32,
        reserved: u32,
        comm: [u8; 16],
        name: [u8; 32],
        files: u32,
        group: u32,
        job: u32,
        device: u32,
        device_group: u32,
        nice: i32,
        start_seconds: u64,
        start_micros: u64,
    }
    #[repr(C)]
    struct TaskInfo {
        virtual_bytes: u64,
        resident_bytes: u64,
        total_user: u64,
        total_system: u64,
        thread_user: u64,
        thread_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        sent: i32,
        received: i32,
        mach_calls: i32,
        unix_calls: i32,
        switches: i32,
        threads: i32,
        running: i32,
        priority: i32,
    }
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            bytes: i32,
        ) -> i32;
    }
    pub struct OwnedProcess {
        pid: u32,
        birth: Option<(u64, u64)>,
    }
    impl OwnedProcess {
        pub fn new(child: &Child) -> Result<Self, QueryError> {
            let pid = child
                .id()
                .ok_or_else(|| QueryError::new("WORKER_ERROR", "native worker PID unavailable"))?;
            let birth = read::<BsdInfo>(pid, 3)?
                .filter(|info| {
                    info.pid == pid && info.parent == std::process::id() && info.status != 5
                })
                .map(|info| (info.start_seconds, info.start_micros));
            Ok(Self { pid, birth })
        }
        pub fn resident_bytes(&self) -> Result<Option<u64>, QueryError> {
            let Some(birth) = self.birth else {
                return Ok(None);
            };
            let matches = |info: &BsdInfo| {
                info.pid == self.pid
                    && info.parent == std::process::id()
                    && info.status != 5
                    && (info.start_seconds, info.start_micros) == birth
            };
            let Some(before) = read::<BsdInfo>(self.pid, 3)? else {
                return Ok(None);
            };
            if !matches(&before) {
                return Ok(None);
            }
            let Some(task) = read::<TaskInfo>(self.pid, 4)? else {
                return Ok(None);
            };
            let Some(after) = read::<BsdInfo>(self.pid, 3)? else {
                return Ok(None);
            };
            if !matches(&after) {
                return Ok(None);
            }
            Ok(Some(task.resident_bytes))
        }
    }
    fn read<T>(pid: u32, flavor: i32) -> Result<Option<T>, QueryError> {
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        let bytes = i32::try_from(std::mem::size_of::<T>())
            .map_err(|_| QueryError::new("WORKER_ERROR", "native process-info layout too large"))?;
        // SAFETY: only verified C-layout structs are passed, with a writable
        // correctly sized/aligned buffer; no value is read before a full fill.
        let result =
            unsafe { proc_pidinfo(pid as i32, flavor, 0, value.as_mut_ptr().cast(), bytes) };
        if result == 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ESRCH | libc::ENOENT)) {
                return Ok(None);
            }
            return Err(query_error(error));
        }
        if result != bytes {
            return Err(QueryError::new(
                "WORKER_ERROR",
                "native process-info layout does not match OS",
            ));
        }
        // SAFETY: the API reported filling exactly the complete struct above.
        Ok(Some(unsafe { value.assume_init() }))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        bytes: u32,
        faults: u32,
        peak_working: usize,
        working: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn DuplicateHandle(
            source_process: *mut std::ffi::c_void,
            source: *mut std::ffi::c_void,
            target_process: *mut std::ffi::c_void,
            target: *mut *mut std::ffi::c_void,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        fn K32GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            bytes: u32,
        ) -> i32;
        fn GetExitCodeProcess(process: *mut std::ffi::c_void, code: *mut u32) -> i32;
    }
    pub struct OwnedProcess {
        handle: OwnedHandle,
    }
    impl OwnedProcess {
        pub fn new(child: &Child) -> Result<Self, QueryError> {
            let original = child.raw_handle().ok_or_else(|| {
                QueryError::new("WORKER_ERROR", "native worker process handle unavailable")
            })?;
            let mut handle = std::ptr::null_mut();
            // SAFETY: child owns a live process handle; DuplicateHandle creates
            // an independent non-inherited owned handle to that exact process.
            let status = unsafe {
                let process = GetCurrentProcess();
                DuplicateHandle(process, original, process, &mut handle, 0, 0, 2)
            };
            if status == 0 {
                return Err(query_error(std::io::Error::last_os_error()));
            }
            // SAFETY: the successful duplication transferred this new handle to us.
            Ok(Self {
                handle: unsafe { OwnedHandle::from_raw_handle(handle) },
            })
        }
        pub fn resident_bytes(&self) -> Result<Option<u64>, QueryError> {
            let mut code = 0;
            // SAFETY: handle remains owned and code is writable output storage.
            if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
                return Err(query_error(std::io::Error::last_os_error()));
            }
            if code != 259 {
                return Ok(None);
            } // STILL_ACTIVE.
            let bytes = std::mem::size_of::<Counters>() as u32;
            let mut counters = Counters {
                bytes,
                ..Default::default()
            };
            // SAFETY: handle is the exact owned worker, buffer has verified C
            // layout and size, and it is read only after API success.
            if unsafe { K32GetProcessMemoryInfo(self.handle.as_raw_handle(), &mut counters, bytes) }
                == 0
            {
                return Err(query_error(std::io::Error::last_os_error()));
            }
            Ok(Some(counters.working as u64))
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
mod platform {
    use super::*;
    pub struct OwnedProcess;
    impl OwnedProcess {
        pub fn new(_child: &Child) -> Result<Self, QueryError> {
            Err(QueryError::new(
                "WORKER_ERROR",
                "native worker RSS monitoring unsupported on this platform",
            ))
        }
        pub fn resident_bytes(&self) -> Result<Option<u64>, QueryError> {
            Ok(None)
        }
    }
}
