// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Check T8122 OPP voltages against this boot's iBoot ADT before GPU startup.
//!
//! This validates data provenance and shape only. A separate gate must ground
//! the power table and leakage coefficients before enabling the GPU.

use kernel::{c_str, device::Core, platform, prelude::*};

const PSTATE_COUNT: usize = 14;
const STATE_BYTES: usize = 8;

fn adt_u32(node: &kernel::of::Node, name: &CStr) -> Result<u32> {
    let bytes: KVec<u8> = node.get_property(name)?;
    if bytes.len() != 4 {
        return Err(EINVAL);
    }
    Ok(u32::from_le_bytes(bytes.as_slice().try_into().map_err(|_| EINVAL)?))
}

fn state(bytes: &[u8], index: usize) -> Result<(u32, u32)> {
    let entry = bytes
        .get(index * STATE_BYTES..(index + 1) * STATE_BYTES)
        .ok_or(EINVAL)?;
    let hz = u32::from_le_bytes(entry[..4].try_into().map_err(|_| EINVAL)?);
    let mv = u32::from_le_bytes(entry[4..].try_into().map_err(|_| EINVAL)?);
    Ok((hz, mv))
}

pub(crate) fn validate_opp(pdev: &platform::Device<Core>) -> Result<()> {
    let gpu = pdev.as_ref().of_node().ok_or(ENODEV)?;
    let diagnostic = kernel::of::chosen()
        .and_then(|chosen| chosen.get_child_by_name(c_str!("asahi,t8122-gpu-diagnostics")))
        .ok_or(ENODEV)?;
    let sgx = diagnostic.get_child_by_name(c_str!("sgx")).ok_or(ENODEV)?;
    let core: KVec<u8> = sgx.get_property(c_str!("adt,perf-states"))?;
    let sram: KVec<u8> = sgx.get_property(c_str!("adt,perf-states-sram"))?;
    if adt_u32(&sgx, c_str!("adt,perf-state-count"))? != PSTATE_COUNT as u32
        || adt_u32(&sgx, c_str!("adt,gpu-num-perf-states"))? != PSTATE_COUNT as u32 - 1
        || adt_u32(&sgx, c_str!("adt,perf-state-table-count"))? != 1
        || core.len() != PSTATE_COUNT * STATE_BYTES
        || sram.len() != PSTATE_COUNT * STATE_BYTES
    {
        return Err(EINVAL);
    }

    let opps = gpu
        .parse_phandle(c_str!("operating-points-v2"), 0)
        .ok_or(ENODEV)?;
    let mut count = 0;
    let mut min_sram_mv = u32::MAX;
    let mut previous_power = 0;
    for opp in opps.children() {
        if count >= PSTATE_COUNT {
            return Err(EINVAL);
        }
        let (core_hz, core_mv) = state(core.as_slice(), count)?;
        let (sram_hz, sram_mv) = state(sram.as_slice(), count)?;
        let core_uv = core_mv.checked_mul(1000).ok_or(EINVAL)?;
        let hz: u64 = opp.get_property(c_str!("opp-hz"))?;
        let uv: KVec<u32> = opp.get_property(c_str!("opp-microvolt"))?;
        let power_uw: u32 = opp.get_property(c_str!("opp-microwatt"))?;
        if core_hz != sram_hz
            || hz != u64::from(core_hz)
            || uv.as_slice() != [core_uv]
            || sram_mv < core_mv
            || core_mv == 0
            || core_mv > 1500
            || sram_mv > 1500
            || (count > 0 && !(100_000_000..=2_000_000_000).contains(&core_hz))
            || (count == 0 && power_uw != 0)
            || (count > 0 && (power_uw == 0 || power_uw < previous_power))
        {
            return Err(EINVAL);
        }
        if count > 0 {
            min_sram_mv = min_sram_mv.min(sram_mv);
        }
        previous_power = power_uw;
        count += 1;
    }
    let floor: u32 = gpu.get_property(c_str!("apple,min-sram-microvolt"))?;
    if count != PSTATE_COUNT
        || floor != min_sram_mv.checked_mul(1000).ok_or(EINVAL)?
    {
        return Err(EINVAL);
    }
    Ok(())
}
