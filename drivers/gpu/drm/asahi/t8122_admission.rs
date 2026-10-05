// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Read-only T8122 identity admission. Firmware startup remains disabled.

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) active_cores: u32,
    pub(crate) core_slots: u32,
    pub(crate) mgpu_clusters: u32,
    pub(crate) gps_per_cluster: u32,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Board {
    J613,
    Unvalidated,
}

/// Admit only the bounded stage-2 result and G15G revision observed on M3 Air.
/// The ten-bit field describes core *slots*, not ten active cores: J613 has eight.
/// The mask and count words below were measured on this J613 under a bounded
/// stage-2 power vote; reject other topologies until separately validated.
pub(crate) fn validate_identity(
    board: Board,
    firmware_compat: &[u32],
    schema: u32,
    result: u32,
    id_version: u32,
    id_counts_1: u32,
    id_counts_2: u32,
    masks: [u32; 3],
) -> Option<Identity> {
    let family = id_version >> 24;
    let variant = (id_version >> 16) & 0xff;
    let revision = (id_version >> 8) & 0xff;
    let mgpu_clusters = (id_counts_1 >> 16) & 0xf;
    let core_slots = id_counts_1 & 0xff;
    let gps_per_cluster = (id_counts_2 >> 16) & 0xff;
    let active_cores = masks[0].count_ones();
    if board != Board::J613
        || firmware_compat != [14, 8, 3]
        || schema != 1
        || result != 1
        || family != 7
        || variant != 2
        || revision != 0x20
        || id_counts_1 != 0x0011_010a
        || id_counts_2 != 0x0004_0404
        || masks[0] != 0x37e
        || masks[1] != 0
        || masks[2] != 0
        || active_cores != 8
    {
        return None;
    }
    Some(Identity {
        active_cores,
        core_slots,
        mgpu_clusters,
        gps_per_cluster,
    })
}

/// Read the m1n1 observation and firmware ABI before any GPU MMIO or DMA setup.
/// This is intentionally only an identity gate; it does not admit a full HwConfig.
#[cfg(not(test))]
pub(crate) fn read_identity(
    pdev: &kernel::platform::Device<kernel::device::Core>,
) -> kernel::error::Result<Identity> {
    use kernel::{bindings, c_str, prelude::*};

    struct NodeRef(*mut bindings::device_node);
    impl Drop for NodeRef {
        fn drop(&mut self) {
            // SAFETY: `of_find_node_opts_by_path` returned this owned reference.
            unsafe { bindings::of_node_put(self.0) };
        }
    }
    impl NodeRef {
        fn get_u32(&self, name: &CStr) -> Result<u32> {
            let mut length = 0;
            // SAFETY: `self` holds the node reference; the property pointer is
            // read only while that reference remains live.
            let value =
                unsafe { bindings::of_get_property(self.0, name.as_char_ptr(), &mut length) };
            if value.is_null() || length != 4 {
                return Err(ENODEV);
            }
            // SAFETY: the property is exactly four bytes, checked above.
            let bytes = unsafe { core::slice::from_raw_parts(value as *const u8, 4) };
            Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        }
    }

    let gpu = pdev.as_ref().of_node().ok_or(ENODEV)?;
    let board = kernel::of::root()
        .and_then(|root| root.get_property::<KVec<u8>>(c_str!("compatible")).ok())
        .map(|compatible| {
            if compatible
                .split(|byte| *byte == 0)
                .any(|entry| entry == b"apple,j613")
            {
                Board::J613
            } else {
                Board::Unvalidated
            }
        })
        .unwrap_or(Board::Unvalidated);
    let firmware_compat: KVec<u32> = gpu.get_property(c_str!("apple,firmware-compat"))?;
    let path = c_str!("/chosen/asahi,t8122-gpu-powered-identity");
    // SAFETY: `path` is a static NUL-terminated string. A non-null result is
    // an owned OF reference released by `NodeRef`.
    let raw =
        unsafe { bindings::of_find_node_opts_by_path(path.as_char_ptr(), core::ptr::null_mut()) };
    if raw.is_null() {
        return Err(ENODEV);
    }
    let observation = NodeRef(raw);
    let schema = observation.get_u32(c_str!("schema-version"))?;
    let result = observation.get_u32(c_str!("probe-result"))?;
    let id_version = observation.get_u32(c_str!("id-version"))?;
    let id_counts_1 = observation.get_u32(c_str!("id-counts-1"))?;
    let id_counts_2 = observation.get_u32(c_str!("id-counts-2"))?;
    let masks = [
        observation.get_u32(c_str!("core-mask-0"))?,
        observation.get_u32(c_str!("core-mask-1"))?,
        observation.get_u32(c_str!("core-mask-2"))?,
    ];
    validate_identity(
        board,
        firmware_compat.as_slice(),
        schema,
        result,
        id_version,
        id_counts_1,
        id_counts_2,
        masks,
    )
    .ok_or(ENODEV)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G15G_C0: u32 = 0x0702_2000;
    const COUNTS_1: u32 = 0x0011_010a;
    const COUNTS_2: u32 = 0x0004_0404;

    #[test]
    fn admits_measured_j613_count_without_conflating_slots() {
        let id = validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0x37e, 0, 0],
        )
        .unwrap();
        assert_eq!(id.active_cores, 8);
        assert_eq!(id.core_slots, 10);
        assert_eq!(id.mgpu_clusters, 1);
        assert_eq!(id.gps_per_cluster, 4);
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0xff, 0, 0]
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1 ^ 0x100,
            COUNTS_2,
            [0x37e, 0, 0]
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0x3ff, 0, 0]
        )
        .is_none());
        assert!(validate_identity(
            Board::Unvalidated,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0x37e, 0, 0]
        )
        .is_none());
    }

    #[test]
    fn rejects_failed_probes_wrong_revision_and_out_of_range_masks() {
        let valid = [0x37e, 0, 0];
        assert!(validate_identity(
            Board::J613,
            &[14, 7, 0],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            valid
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            2,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            valid
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            5,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            valid
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            0x0703_2000,
            COUNTS_1,
            COUNTS_2,
            valid
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            0x0702_1000,
            COUNTS_1,
            COUNTS_2,
            valid
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0x4ff, 0, 0]
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0xff, 1, 0]
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0xff, 0, 1]
        )
        .is_none());
        assert!(validate_identity(
            Board::J613,
            &[14, 8, 3],
            1,
            1,
            G15G_C0,
            COUNTS_1,
            COUNTS_2,
            [0x7f, 0, 0]
        )
        .is_none());
    }
}
