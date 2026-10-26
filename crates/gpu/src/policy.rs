//! Which device renders: one setting for the app and `lightcraft-cli` (Auto / Force GPU / Disable
//! GPU), resolved once at start-up by both from the same inputs, in the same order
//! ([`resolve_policy`]), so the same photo and settings export the same bytes from either.
//!
//! - **Auto** (default): the GPU when there is a usable one; a render the GPU can't do (16-bit,
//!   LUT profile, refined mask, soft proof) or that fails on it (out of memory, device limit, blank
//!   readback, device lost) renders on the CPU, and the export reports the fallback.
//! - **Force GPU**: never the CPU for a render that would use the GPU under Auto. A render the GPU
//!   can't do, or that fails on it, is an error that says why. The CLI also fails at start-up when
//!   no device can be created.
//! - **Disable GPU**: no GPU compute at all. Set before anything can create a device, so no adapter
//!   is enumerated and no driver is loaded for compute (render, denoise, faces, SAM 3); issues
//!   #78, #315, #410. (The app's window has its own renderer: see `docs/gpu-pipeline.md`.)

use std::sync::atomic::{AtomicU8, Ordering};

/// The device policy (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GpuPolicy {
    #[default]
    Auto,
    Force,
    Disable,
}

impl GpuPolicy {
    /// `auto`; `gpu` / `force` / `on`; `cpu` / `off` / `disable` / `0` / `false` / `no` (any case).
    pub fn parse(s: &str) -> Option<GpuPolicy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatic" => Some(GpuPolicy::Auto),
            "gpu" | "force" | "on" => Some(GpuPolicy::Force),
            "cpu" | "off" | "disable" | "disabled" | "none" | "0" | "false" | "no" => Some(GpuPolicy::Disable),
            _ => None,
        }
    }

    /// The canonical spelling: `auto`, `gpu` or `cpu` (what [`GpuPolicy::parse`] reads back).
    pub fn name(self) -> &'static str {
        match self {
            GpuPolicy::Auto => "auto",
            GpuPolicy::Force => "gpu",
            GpuPolicy::Disable => "cpu",
        }
    }

    fn code(self) -> u8 {
        match self {
            GpuPolicy::Auto => 0,
            GpuPolicy::Force => 1,
            GpuPolicy::Disable => 2,
        }
    }
}

static POLICY: AtomicU8 = AtomicU8::new(0);

/// Set the process's device policy. Disable takes effect before the next device would be created
/// (call it before any render); a device that already exists is no longer used for renders.
pub fn set_policy(p: GpuPolicy) {
    POLICY.store(p.code(), Ordering::Relaxed);
}

/// The process's device policy as set ([`set_policy`]); `LIGHTCRAFT_GPU=0` turns the GPU off
/// regardless (see [`crate::enabled`]).
pub fn policy() -> GpuPolicy {
    match POLICY.load(Ordering::Relaxed) {
        1 => GpuPolicy::Force,
        2 => GpuPolicy::Disable,
        _ => GpuPolicy::Auto,
    }
}

/// Whether renders that would use the GPU must not fall back to the CPU.
pub fn forced() -> bool {
    policy() == GpuPolicy::Force
}

/// What `LIGHTCRAFT_GPU` / `LIGHTCRAFT_GPU_BACKEND` ask for, if anything. The off values are a hard
/// switch for the whole process; `auto` and `gpu` are read like the `--gpu` flag. Other values
/// (e.g. `1`) mean nothing, as before.
pub fn env_policy() -> Option<(GpuPolicy, &'static str)> {
    if let Some(why) = crate::env_off_reason() {
        return Some((GpuPolicy::Disable, why));
    }
    match std::env::var("LIGHTCRAFT_GPU").ok().as_deref().map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("auto" | "automatic") => Some((GpuPolicy::Auto, "LIGHTCRAFT_GPU=auto")),
        Some("gpu" | "force") => Some((GpuPolicy::Force, "LIGHTCRAFT_GPU=gpu")),
        _ => None,
    }
}

/// A resolved policy and where it came from (`--gpu flag`, `LIGHTCRAFT_GPU=…`, `saved setting`,
/// `default`, …), with a note to log when an input was overridden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub policy: GpuPolicy,
    pub source: String,
    pub note: Option<String>,
}

/// The device policy for this process, decided the same way by the app and the CLI:
///
/// 1. the command-line flag (`--gpu auto|gpu|cpu`);
/// 2. the environment (`LIGHTCRAFT_GPU`; its off values and `LIGHTCRAFT_GPU_BACKEND=off` turn the
///    GPU off whatever the flag says, and a `--gpu gpu` flag against them is an error);
/// 3. the saved setting (`gpu` and `gpuForce` in the app's `ui.json`);
/// 4. Auto.
///
/// Then the crash markers (`crash`: the last start died creating the GPU device, or the device was
/// lost): Auto becomes Disable for this run (with a note); Force is an error, never a silent
/// fallback.
pub fn resolve_policy(flag: Option<GpuPolicy>, saved: Option<GpuPolicy>, crash: Option<&str>) -> Result<Resolved, String> {
    resolve_with(flag, env_policy(), saved, crash)
}

pub(crate) fn resolve_with(
    flag: Option<GpuPolicy>,
    env: Option<(GpuPolicy, &str)>,
    saved: Option<GpuPolicy>,
    crash: Option<&str>,
) -> Result<Resolved, String> {
    let r = |policy, source: &str, note: Option<String>| Resolved { policy, source: source.to_string(), note };
    let mut out = match (flag, env) {
        (Some(GpuPolicy::Force), Some((GpuPolicy::Disable, why))) => {
            return Err(format!("--gpu gpu asks for the GPU, but it is {why}; unset it or use --gpu auto / --gpu cpu"));
        }
        (Some(GpuPolicy::Disable), _) => r(GpuPolicy::Disable, "--gpu flag", None),
        (Some(GpuPolicy::Auto), Some((GpuPolicy::Disable, why))) => {
            r(GpuPolicy::Disable, why, Some(format!("--gpu auto: the GPU stays off ({why})")))
        }
        (Some(p), _) => r(p, "--gpu flag", None),
        (None, Some((p, why))) => r(p, why, None),
        (None, None) => match saved {
            Some(p) => r(p, "saved setting", None),
            None => r(GpuPolicy::Auto, "default", None),
        },
    };
    if let Some(what) = crash {
        match out.policy {
            GpuPolicy::Auto => {
                out = r(GpuPolicy::Disable, "crash marker", Some(format!("the GPU failed last time ({what}): rendering on the CPU this run")));
            }
            GpuPolicy::Force => {
                return Err(format!(
                    "the GPU failed last time ({what}); Force GPU won't run until LightCraft starts once with the GPU on Automatic or off \
                     (that clears the marker), or run with --gpu auto / --gpu cpu"
                ));
            }
            GpuPolicy::Disable => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use GpuPolicy::*;

    #[test]
    fn names_read_back() {
        for p in [Auto, Force, Disable] {
            assert_eq!(GpuPolicy::parse(p.name()), Some(p));
        }
        assert_eq!(GpuPolicy::parse("Force"), Some(Force));
        assert_eq!(GpuPolicy::parse("0"), Some(Disable));
        assert_eq!(GpuPolicy::parse("1"), None);
    }

    #[test]
    fn flag_then_env_then_saved_then_default() {
        let off = Some((Disable, "disabled by LIGHTCRAFT_GPU=0"));
        assert_eq!(resolve_with(None, None, None, None).map(|r| r.policy), Ok(Auto));
        assert_eq!(resolve_with(None, None, Some(Disable), None).map(|r| r.policy), Ok(Disable));
        assert_eq!(resolve_with(None, Some((Force, "LIGHTCRAFT_GPU=gpu")), Some(Disable), None).map(|r| r.policy), Ok(Force));
        assert_eq!(resolve_with(Some(Auto), Some((Force, "LIGHTCRAFT_GPU=gpu")), Some(Disable), None).map(|r| r.policy), Ok(Auto));
        assert_eq!(resolve_with(Some(Disable), None, Some(Force), None).map(|r| r.policy), Ok(Disable));
        // the environment's off switch is hard
        assert_eq!(resolve_with(Some(Auto), off, None, None).map(|r| r.policy), Ok(Disable));
        assert_eq!(resolve_with(None, off, Some(Force), None).map(|r| r.policy), Ok(Disable));
        assert!(resolve_with(Some(Force), off, None, None).is_err());
    }

    #[test]
    fn a_crash_turns_auto_off_and_stops_force() {
        let r = resolve_with(None, None, None, Some("device lost")).unwrap_or_else(unreachable_ok);
        assert_eq!((r.policy, r.source.as_str()), (Disable, "crash marker"));
        assert!(resolve_with(Some(Force), None, None, Some("device lost")).is_err());
        assert_eq!(resolve_with(None, None, Some(Disable), Some("x")).map(|r| r.policy), Ok(Disable));
    }

    fn unreachable_ok(e: String) -> Resolved {
        Resolved { policy: Force, source: e, note: None }
    }
}
