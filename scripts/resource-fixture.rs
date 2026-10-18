// Standalone native fixture, compiled directly by rustc (no product dependencies).
use std::{ffi::c_void, fs, hint::black_box, process::Command, thread, time::Duration};
use std::os::windows::process::CommandExt;
type Handle = *mut c_void;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn GetProcessTimes(process: Handle, created: *mut i64, exited: *mut i64, kernel: *mut i64, user: *mut i64) -> i32;
    fn CreateJobObjectW(attributes: Handle, name: *const u16) -> Handle;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}
fn cpu() -> i64 {
    let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
    assert_ne!(unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }, 0);
    kernel + user
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let report = &args[1];
    if args.get(2).map(String::as_str) == Some("child") {
        let initial = cpu();
        let started = std::time::Instant::now();
        let mut value = 0.0;
        while cpu() - initial < 2_200_000 {
            for item in 0..20_000 { value += black_box(f64::sqrt(item as f64 + 1.0)); }
        }
        fs::write(report, format!("{{\"childCpu100ns\":{},\"childWallSeconds\":{},\"childProcessId\":{},\"value\":{}}}", cpu()-initial, started.elapsed().as_secs_f64(), std::process::id(), value)).unwrap();
        return;
    }
    let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
    assert!(!job.is_null());
    assert_ne!(unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) }, 0);
    thread::sleep(Duration::from_millis(250));
    let status = Command::new(std::env::current_exe().unwrap()).args([report, "child"]).creation_flags(0x08000000).status().unwrap();
    assert!(status.success());
    fs::write(format!("{}.parent.json", report), "{\"nestedJob\":true}").unwrap();
    thread::sleep(Duration::from_secs(12));
    unsafe { CloseHandle(job); }
}
