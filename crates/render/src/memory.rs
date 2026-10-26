//! The process's memory as macOS accounts it: the physical footprint (what Activity Monitor shows and `/usr/bin/time -l`
//! reports as its peak), the largest it has been, and how much of it is the GPU's (the graphics ledger: textures and
//! buffers the process owns that are not mapped into it). Elsewhere, nothing yet. And the budget the resident maps
//! share ([`budget`]).

/// One reading, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    pub now: u64,
    /// The largest `now` has been since the process started.
    pub peak: u64,
    /// Of `now`, the graphics ledger's: what Metal holds for the process on the GPU's side.
    pub graphics: u64,
}

impl Footprint {
    pub fn now_mb(&self) -> f64 {
        mb(self.now)
    }
}

pub fn mb(bytes: u64) -> f64 {
    (bytes as f64 / 1e6 * 10.0).round() / 10.0
}

#[cfg(target_os = "macos")]
mod mach {
    /// `struct task_vm_info` up to revision 4 (mach/task_info.h): every field is 8 bytes after the two `integer_t`s,
    /// so `#pragma pack(4)` lays it out as `repr(C)` does.
    #[repr(C)]
    #[derive(Default)]
    pub struct TaskVmInfo {
        pub virtual_size: u64,
        pub region_count: i32,
        pub page_size: i32,
        pub resident_size: u64,
        pub resident_size_peak: u64,
        pub device: u64,
        pub device_peak: u64,
        pub internal: u64,
        pub internal_peak: u64,
        pub external: u64,
        pub external_peak: u64,
        pub reusable: u64,
        pub reusable_peak: u64,
        pub purgeable_volatile_pmap: u64,
        pub purgeable_volatile_resident: u64,
        pub purgeable_volatile_virtual: u64,
        pub compressed: u64,
        pub compressed_peak: u64,
        pub compressed_lifetime: u64,
        pub phys_footprint: u64,
        pub min_address: u64,
        pub max_address: u64,
        pub ledger_phys_footprint_peak: i64,
        pub ledger_purgeable_nonvolatile: i64,
        pub ledger_purgeable_novolatile_compressed: i64,
        pub ledger_purgeable_volatile: i64,
        pub ledger_purgeable_volatile_compressed: i64,
        pub ledger_tag_network_nonvolatile: i64,
        pub ledger_tag_network_nonvolatile_compressed: i64,
        pub ledger_tag_network_volatile: i64,
        pub ledger_tag_network_volatile_compressed: i64,
        pub ledger_tag_media_footprint: i64,
        pub ledger_tag_media_footprint_compressed: i64,
        pub ledger_tag_media_nofootprint: i64,
        pub ledger_tag_media_nofootprint_compressed: i64,
        pub ledger_tag_graphics_footprint: i64,
        pub ledger_tag_graphics_footprint_compressed: i64,
        pub ledger_tag_graphics_nofootprint: i64,
        pub ledger_tag_graphics_nofootprint_compressed: i64,
        pub ledger_tag_neural_footprint: i64,
        pub ledger_tag_neural_footprint_compressed: i64,
        pub ledger_tag_neural_nofootprint: i64,
        pub ledger_tag_neural_nofootprint_compressed: i64,
        pub limit_bytes_remaining: u64,
    }

    pub const TASK_VM_INFO: u32 = 22;

    unsafe extern "C" {
        static mach_task_self_: u32;
        pub fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
    }

    pub fn task_self() -> u32 {
        // SAFETY: the kernel sets this port before any of the process's code runs and never changes it
        unsafe { mach_task_self_ }
    }
}

/// The process's footprint now; None where it cannot be read.
#[cfg(target_os = "macos")]
pub fn footprint() -> Option<Footprint> {
    let mut info = mach::TaskVmInfo::default();
    let mut count = (std::mem::size_of::<mach::TaskVmInfo>() / 4) as u32;
    // SAFETY: task_info writes at most `count` 4-byte words into `info`, which is that large, and says how many it wrote
    let status = unsafe { mach::task_info(mach::task_self(), mach::TASK_VM_INFO, (&raw mut info).cast(), &raw mut count) };
    if status != 0 {
        return None;
    }
    // revision 3 (the ledgers) is 66 words in; an older kernel leaves them zero
    let graphics = (info.ledger_tag_graphics_footprint + info.ledger_tag_graphics_footprint_compressed).max(0) as u64;
    let peak = u64::try_from(info.ledger_phys_footprint_peak).unwrap_or(0).max(info.phys_footprint);
    Some(Footprint { now: info.phys_footprint, peak, graphics })
}

#[cfg(not(target_os = "macos"))]
pub fn footprint() -> Option<Footprint> {
    None
}

/// What the resident maps may hold on the GPU by default: 8 GiB, enough for all seven pool maps (6.2 GB together).
pub const DEFAULT_BUDGET: u64 = 8 << 30;
/// Where the GPU does not say how much it can keep resident.
pub const FALLBACK_BUDGET: u64 = 4 << 30;

/// The maps' budget: what was asked for, else [`DEFAULT_BUDGET`] but no more than half of what the GPU recommends a
/// process keep resident (Metal's `recommendedMaxWorkingSetSize`, ≈ ¾ of the machine's memory on Apple silicon), else
/// [`FALLBACK_BUDGET`].
pub fn budget(asked: Option<u64>, recommended: Option<u64>) -> u64 {
    asked.unwrap_or_else(|| recommended.map_or(FALLBACK_BUDGET, |r| DEFAULT_BUDGET.min(r / 2)))
}

/// How much the GPU recommends this process keep resident (Metal); None elsewhere.
pub fn recommended_working_set(device: &wgpu::Device) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        use objc2_metal::MTLDevice as _;
        // SAFETY: only reads a property of the device
        unsafe { device.as_hal::<wgpu::hal::api::Metal>().map(|d| d.raw_device().recommendedMaxWorkingSetSize()) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = device;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_asked_for_or_bounded_by_the_gpu() {
        assert_eq!(budget(Some(3 << 30), Some(96 << 30)), 3 << 30);
        // a large machine: the default; a 16 GB one (10.7 GB recommended): half of it
        assert_eq!(budget(None, Some(96 << 30)), DEFAULT_BUDGET);
        assert_eq!(budget(None, Some(10_700 << 20)), 5_350 << 20);
        assert_eq!(budget(None, None), FALLBACK_BUDGET);
    }

    #[test]
    fn memory_is_read_and_grows_with_what_the_process_holds() {
        let Some(before) = footprint() else { return };
        assert!(before.now > 0 && before.peak >= before.now, "{before:?}");
        // 64 MB written: dirty pages the footprint counts
        let held = vec![1u8; 64 << 20];
        std::hint::black_box(&held);
        let after = footprint().expect("readable");
        std::hint::black_box(&held);
        assert!(after.now >= before.now + (48 << 20), "{before:?} → {after:?}");
        assert!(after.peak >= after.now);
        drop(held);
        assert_eq!(mb(1_234_567), 1.2);
    }
}
