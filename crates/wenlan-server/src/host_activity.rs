// SPDX-License-Identifier: Apache-2.0
//! Low-overhead host signals used only to veto new background-heavy work.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum HostActivitySnapshot {
    /// The current platform has no production probe; portable CPU/RAM admission
    /// remains authoritative.
    #[cfg(any(not(target_os = "macos"), test))]
    Unsupported,
    /// A supported platform failed to produce trustworthy telemetry.
    Unavailable,
    Observed {
        thermal_state: u8,
        idle_for: Duration,
    },
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn wenlan_macos_thermal_state() -> i32;
    fn wenlan_macos_seconds_since_last_input() -> f64;
    fn wenlan_macos_battery_percent_on_battery() -> i32;
}

#[cfg(target_os = "macos")]
pub(crate) fn sample_host_activity() -> HostActivitySnapshot {
    // SAFETY: Both functions are zero-argument wrappers compiled into this
    // crate. They synchronously return scalar values from public macOS APIs and
    // retain no Rust pointers or callbacks.
    let (thermal_state, seconds_since_input) = unsafe {
        (
            wenlan_macos_thermal_state(),
            wenlan_macos_seconds_since_last_input(),
        )
    };
    let Ok(thermal_state) = u8::try_from(thermal_state) else {
        return HostActivitySnapshot::Unavailable;
    };
    if thermal_state > 3 {
        return HostActivitySnapshot::Unavailable;
    }
    let Ok(idle_for) = Duration::try_from_secs_f64(seconds_since_input) else {
        return HostActivitySnapshot::Unavailable;
    };

    HostActivitySnapshot::Observed {
        thermal_state,
        idle_for,
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sample_host_activity() -> HostActivitySnapshot {
    HostActivitySnapshot::Unsupported
}

/// On battery, background work that runs a model on this computer stops
/// below this charge and starts again at [`BATTERY_OKAY_PERCENT`]
/// (UX-QUIET-1). These are Android's defaults (AOSP config.xml
/// `config_lowBatteryWarningLevel` 20, cleared at +5), which also drive
/// JobScheduler's "battery not low"; Windows Battery Saver also turns on
/// at 20%.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const LOW_BATTERY_PERCENT: i32 = 20;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const BATTERY_OKAY_PERCENT: i32 = 25;

/// Whether the battery counts as low, given the charge (`None` on AC power
/// or unreadable) and whether it counted as low last time. The 5-point gap
/// keeps work from flapping on and off around one number.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn battery_is_low(percent_on_battery: Option<i32>, was_low: bool) -> bool {
    match percent_on_battery {
        None => false,
        Some(percent) if was_low => percent < BATTERY_OKAY_PERCENT,
        Some(percent) => percent < LOW_BATTERY_PERCENT,
    }
}

#[cfg(target_os = "macos")]
static BATTERY_WAS_LOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the computer is on battery with a low charge (see
/// [`battery_is_low`]). AC power, a desktop, an unreadable power source, and
/// every platform without a probe count as "not low", so this can only hold
/// work back on a laptop that reports a low battery.
#[cfg(target_os = "macos")]
pub(crate) fn sample_low_battery() -> bool {
    use std::sync::atomic::Ordering;
    // SAFETY: zero-argument wrapper compiled into this crate; it returns a
    // scalar and retains nothing.
    let percent = unsafe { wenlan_macos_battery_percent_on_battery() };
    let low = battery_is_low(
        (percent >= 0).then_some(percent),
        BATTERY_WAS_LOW.load(Ordering::Relaxed),
    );
    BATTERY_WAS_LOW.store(low, Ordering::Relaxed);
    low
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sample_low_battery() -> bool {
    false
}
