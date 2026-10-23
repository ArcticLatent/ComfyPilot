//! Windows process-tree lifetime guard for managed ComfyUI instances.
//!
//! A Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` makes Windows
//! terminate ComfyUI and its descendants if the helper exits abruptly. The
//! handle remains owned here until the runtime explicitly releases it.

use std::sync::Mutex;

static COMFY_JOB_OBJECT: Mutex<Option<ComfyJobObject>> = Mutex::new(None);

struct ComfyJobObject(windows::Win32::Foundation::HANDLE);

// SAFETY: a Win32 HANDLE is an opaque identifier and the Job Object APIs may
// be called from any thread. Access to the handle is serialized by the mutex.
unsafe impl Send for ComfyJobObject {}

impl Drop for ComfyJobObject {
    fn drop(&mut self) {
        // Closing the last handle triggers KILL_ON_JOB_CLOSE for every process
        // still assigned to the job.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn bind_child_to_job_object(child: &std::process::Child) -> Result<ComfyJobObject, String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    unsafe {
        let job = CreateJobObjectW(None, None).map_err(|err| format!("CreateJobObjectW: {err}"))?;

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        if let Err(err) = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) {
            let _ = CloseHandle(job);
            return Err(format!("SetInformationJobObject: {err}"));
        }

        let process_handle = HANDLE(child.as_raw_handle());
        if let Err(err) = AssignProcessToJobObject(job, process_handle) {
            let _ = CloseHandle(job);
            return Err(format!("AssignProcessToJobObject: {err}"));
        }

        Ok(ComfyJobObject(job))
    }
}

/// Binds a managed child to a fresh Job Object. Failure is best-effort so a
/// machine-specific Job Object issue never prevents ComfyUI from starting.
pub(crate) fn track_comfy_job_object(child: &std::process::Child) {
    match bind_child_to_job_object(child) {
        Ok(job) => {
            if let Ok(mut guard) = COMFY_JOB_OBJECT.lock() {
                *guard = Some(job);
            }
        }
        Err(err) => {
            log::warn!(
                "Failed to bind ComfyUI process to a Job Object (orphan-process protection \
                 won't apply this run): {err}"
            );
        }
    }
}

/// Drops the tracked handle, terminating any processes still assigned to it.
pub(crate) fn release_comfy_job_object() {
    if let Ok(mut guard) = COMFY_JOB_OBJECT.lock() {
        *guard = None;
    }
}
