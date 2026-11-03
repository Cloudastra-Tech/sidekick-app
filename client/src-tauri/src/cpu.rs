//! Process CPU usage, for the Phase 2 "CPU < 15% during a call" check.
//!
//! Reported like Task Manager: the share of *total* machine capacity (all logical cores)
//! used by this process. Covers the Rust core (capture, STT streaming, triggers, coach);
//! the WebView2 UI runs in separate msedgewebview2 processes and is not included.

use std::time::Instant;

pub struct CpuSampler {
    last_wall: Instant,
    last_cpu_100ns: u64,
    logical_cores: f64,
}

impl CpuSampler {
    pub fn new() -> Option<Self> {
        Some(Self {
            last_wall: Instant::now(),
            last_cpu_100ns: process_cpu_100ns()?,
            logical_cores: std::thread::available_parallelism()
                .map(|n| n.get() as f64)
                .unwrap_or(1.0),
        })
    }

    /// Average CPU percent (0–100 of total capacity) since the previous sample.
    pub fn sample(&mut self) -> Option<f64> {
        let now = Instant::now();
        let cpu = process_cpu_100ns()?;
        let wall_100ns = now.duration_since(self.last_wall).as_nanos() as f64 / 100.0;
        let used_100ns = cpu.saturating_sub(self.last_cpu_100ns) as f64;
        self.last_wall = now;
        self.last_cpu_100ns = cpu;
        (wall_100ns > 0.0).then(|| 100.0 * used_100ns / (wall_100ns * self.logical_cores))
    }
}

/// Kernel + user CPU time consumed by this process, in 100 ns units.
#[cfg(windows)]
fn process_cpu_100ns() -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the current-process handle is always valid and the out-pointers are live locals.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }
    let to_u64 = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Some(to_u64(kernel) + to_u64(user))
}

#[cfg(target_os = "macos")]
fn process_cpu_100ns() -> Option<u64> {
    // SAFETY: getrusage only writes into the zeroed struct.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let to_100ns = |t: libc::timeval| t.tv_sec as u64 * 10_000_000 + t.tv_usec as u64 * 10;
    Some(to_100ns(usage.ru_utime) + to_100ns(usage.ru_stime))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn process_cpu_100ns() -> Option<u64> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn busy_loop_registers_cpu_and_sleep_does_not() {
        let mut sampler = CpuSampler::new().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let idle = sampler.sample().unwrap();

        let until = Instant::now() + Duration::from_millis(300);
        let mut x = 0u64;
        while Instant::now() < until {
            x = x.wrapping_add(1);
        }
        let busy = sampler.sample().unwrap();
        assert!(busy > idle, "busy {busy:.2}% should exceed idle {idle:.2}%");
        assert!((0.0..=100.0).contains(&busy));
    }
}
