// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Read-only T8122 firmware and UAT handoff validation. This does not start the GPU.

use crate::g16_resources::{Region, Resources};
use kernel::{c_str, device::Core, platform, prelude::*};

const REGION_NAMES: [&CStr; 6] = [
    c_str!("ttbs"),
    c_str!("pagetables"),
    c_str!("handoff"),
    c_str!("pagetables-l2"),
    c_str!("fw-text"),
    c_str!("fw-data"),
];

fn uuid_from_hex(value: &[u8]) -> Option<[u8; 16]> {
    fn digit(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }
    if value.len() != 33 || value[32] != 0 {
        return None;
    }
    let mut uuid = [0; 16];
    for (index, output) in uuid.iter_mut().enumerate() {
        *output = (digit(value[2 * index])? << 4) | digit(value[2 * index + 1])?;
    }
    Some(uuid)
}

pub(crate) fn read(pdev: &platform::Device<Core>) -> Result<Resources> {
    let gpu = pdev.as_ref().of_node().ok_or(ENODEV)?;
    let root = kernel::of::root().ok_or(ENODEV)?;
    let board: KVec<u8> = root.get_property(c_str!("compatible"))?;
    let compatible: KVec<u8> = gpu.get_property(c_str!("compatible"))?;
    if !board
        .split(|byte| *byte == 0)
        .any(|entry| entry == b"apple,j613")
        || !board
            .split(|byte| *byte == 0)
            .any(|entry| entry == b"apple,t8122")
        || compatible.as_slice() != b"apple,agx-t8122\0"
    {
        return Err(ENODEV);
    }

    let names: KVec<u8> = gpu.get_property(c_str!("memory-region-names"))?;
    let handles: KVec<u32> = gpu.get_property(c_str!("memory-region"))?;
    let reg_names: KVec<u8> = gpu.get_property(c_str!("reg-names"))?;
    if names.as_slice() != b"ttbs\0pagetables\0handoff\0pagetables-l2\0fw-text\0fw-data\0"
        || handles.len() != REGION_NAMES.len()
        || reg_names.as_slice() != b"asc\0sgx\0"
    {
        return Err(EINVAL);
    }
    for (i, handle) in handles.iter().enumerate() {
        if *handle == 0 || handles[..i].contains(handle) {
            return Err(EINVAL);
        }
        let region = gpu
            .parse_phandle(c_str!("memory-region"), i.try_into()?)
            .ok_or(EINVAL)?;
        let no_map = region.get_property::<KVec<u8>>(c_str!("no-map"));
        if no_map.as_ref().is_ok_and(|value| !value.is_empty()) || no_map.is_ok() != (i >= 4) {
            return Err(EINVAL);
        }
    }
    for (name, base, size) in [
        (c_str!("asc"), 0x2_9240_0000, 0x6c000),
        (c_str!("sgx"), 0x2_9000_0000, 0x4000000),
    ] {
        let window = pdev.resource_by_name(name).ok_or(EINVAL)?;
        if window.start() != base || window.size() != size {
            return Err(EINVAL);
        }
    }
    let mboxes: KVec<u32> = gpu.get_property(c_str!("mboxes"))?;
    let mbox = if mboxes.len() == 1 {
        gpu.parse_phandle(c_str!("mboxes"), 0).ok_or(EINVAL)?
    } else {
        return Err(EINVAL);
    };
    let mbox_compatible: KVec<u8> = mbox.get_property(c_str!("compatible"))?;
    let mbox_reg: KVec<u32> = mbox.get_property(c_str!("reg"))?;
    if !mbox_compatible
        .split(|byte| *byte == 0)
        .any(|entry| entry == b"apple,t8122-asc-mailbox")
        || mbox_reg.as_slice() != [2, 0x92408000, 0, 0x4000]
    {
        return Err(EINVAL);
    }

    let mut regions = [Region { base: 0, size: 0 }; 6];
    for (i, name) in REGION_NAMES.iter().enumerate() {
        let resource = gpu.reserved_mem_region_to_resource_byname(name)?;
        regions[i] = Region {
            base: resource.start(),
            size: resource.size(),
        };
    }
    let version: u32 = gpu.get_property(c_str!("apple,m3-handoff-version"))?;
    let vas: KVec<u64> = gpu.get_property(c_str!("apple,firmware-segment-vas"))?;
    let flags: KVec<u32> = gpu.get_property(c_str!("apple,firmware-segment-flags"))?;
    let resources = Resources::validate(
        version,
        regions,
        vas.as_slice().try_into().map_err(|_| EINVAL)?,
        flags.as_slice().try_into().map_err(|_| EINVAL)?,
    )
    .map_err(|_| EINVAL)?;

    let diagnostic = kernel::of::chosen()
        .and_then(|chosen| chosen.get_child_by_name(c_str!("asahi,t8122-gpu-diagnostics")))
        .ok_or(ENODEV)?;
    let complete: KVec<u8> = diagnostic.get_property(c_str!("complete"))?;
    let schema: u32 = diagnostic.get_property(c_str!("schema-version"))?;
    let exporter_revision: u32 = diagnostic.get_property(c_str!("exporter-revision"))?;
    let text_size: u32 = diagnostic.get_property(c_str!("image-text-size"))?;
    let patchbay_offset: u32 = diagnostic.get_property(c_str!("image-patchbay-offset"))?;
    let patchbay_size: u32 = diagnostic.get_property(c_str!("image-patchbay-size"))?;
    let uuid: KVec<u8> = diagnostic.get_property(c_str!("image-uuid"))?;
    let rtkit: KVec<u8> = diagnostic.get_property(c_str!("image-rtkit-version"))?;
    let chosen = kernel::of::chosen().ok_or(ENODEV)?;
    let ack: KVec<u8> = chosen.get_property(c_str!("asahi,t8122-gpu-abi-ack"))?;
    let expected_uuid: KVec<u8> =
        chosen.get_property(c_str!("asahi,t8122-gpu-expected-uuid"))?;
    let expected_rtkit: KVec<u8> =
        chosen.get_property(c_str!("asahi,t8122-gpu-expected-rtkit"))?;
    if !complete.is_empty()
        || schema != 1
        || exporter_revision < 3
        || u64::from(text_size) != resources.regions[4].size
        || patchbay_offset > text_size
        || patchbay_size > text_size - patchbay_offset
        || uuid.len() != 16
        || uuid.iter().all(|byte| *byte == 0)
        || rtkit.len() < 2
        || rtkit[0] == 0
        || *rtkit.last().ok_or(EINVAL)? != 0
        || rtkit[..rtkit.len() - 1].contains(&0)
        || ack.as_slice() != b"14.8.3\0"
        || uuid_from_hex(expected_uuid.as_slice()).as_ref().map(|value| value.as_slice())
            != Some(uuid.as_slice())
        || expected_rtkit.as_slice() != rtkit.as_slice()
    {
        return Err(EINVAL);
    }
    Ok(resources)
}

#[cfg(test)]
mod tests {
    use super::uuid_from_hex;

    #[test]
    fn uuid_hex_has_exact_shape() {
        let hex = b"000102030405060708090a0b0c0d0e0f\0";
        assert_eq!(uuid_from_hex(hex), Some(core::array::from_fn(|index| index as u8)));
        assert_eq!(uuid_from_hex(&hex[..32]), None);
        assert_eq!(uuid_from_hex(b"000102030405060708090a0b0c0d0e0g\0"), None);
    }
}
