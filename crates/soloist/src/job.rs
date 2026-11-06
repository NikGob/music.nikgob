//! Windows: Soloist (and SoundCloud's player host) goes into a job object
//! that kills it when the player's last handle to the job closes, which
//! happens however the player ends (quit, crash, killed), so no Soloist
//! outlives it holding the data dir.
#![allow(unsafe_code)]

use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

struct Job(HANDLE);
// SAFETY: a job handle is a kernel handle usable from any thread.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

fn job() -> Option<&'static Job> {
    static JOB: OnceLock<Option<Job>> = OnceLock::new();
    JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .ok()?;
        Some(Job(job))
    })
    .as_ref()
}

/// Puts process `pid` into the job.
pub fn adopt(pid: u32) {
    let Some(job) = job() else { return };
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) else {
            return;
        };
        if let Err(e) = AssignProcessToJobObject(job.0, process) {
            log::debug!("process {pid} not in the job object: {e}");
        }
        let _ = CloseHandle(process);
    }
}

/// The processes in the job now (those adopted and every process they
/// started, which a job takes in with them); `None` when there is no job or
/// it cannot be asked.
pub fn members() -> Option<Vec<u32>> {
    /// `JOBOBJECT_BASIC_PROCESS_ID_LIST` with room for many ids.
    #[repr(C)]
    struct List {
        assigned: u32,
        listed: u32,
        ids: [usize; 1024],
    }
    let job = job()?;
    let mut list = Box::new(List { assigned: 0, listed: 0, ids: [0; 1024] });
    // SAFETY: the buffer is a `JOBOBJECT_BASIC_PROCESS_ID_LIST` of the size
    // given (its id array longer than the declared one), written by Windows.
    unsafe {
        QueryInformationJobObject(
            Some(job.0),
            JobObjectBasicProcessIdList,
            (&raw mut *list).cast(),
            std::mem::size_of::<List>() as u32,
            None,
        )
        .ok()?;
    }
    let n = (list.listed as usize).min(list.ids.len());
    Some(list.ids[..n].iter().map(|&id| id as u32).collect())
}
