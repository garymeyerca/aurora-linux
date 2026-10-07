// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Host regression tests for the production M3 compute byte encoders.
//! Run from the kernel source root:
//! rustc --edition=2021 --test tools/asahi/m3-compute-tests.rs -o /tmp/m3-compute-tests
//! /tmp/m3-compute-tests
//!
//! Synthetic addresses only. These tests establish bounds and pointer wiring,
//! not firmware acceptance, G15 boot self-test execution, or J613 compatibility.
#![allow(dead_code)]

#[path = "../../drivers/gpu/drm/asahi/m3_compute_layout.rs"]
mod m3_compute_layout;
#[path = "../../drivers/gpu/drm/asahi/m3_compute_sequence.rs"]
mod m3_compute_sequence;
#[path = "../../drivers/gpu/drm/asahi/m3_compute_storage.rs"]
mod m3_compute_storage;
#[path = "../../drivers/gpu/drm/asahi/m3_init_layout.rs"]
mod m3_init_layout;
#[path = "../../drivers/gpu/drm/asahi/m3_pool_layout.rs"]
mod m3_pool_layout;
#[path = "../../drivers/gpu/drm/asahi/m3_queue_layout.rs"]
mod m3_queue_layout;

use m3_compute_layout::{Command, GpuVa};
use m3_compute_sequence::Sequence;
use m3_init_layout::Region;
use m3_pool_layout::GpuRegion;
use m3_queue_layout::FirmwareVa;
use {m3_compute_layout as cmd, m3_compute_sequence as seq, m3_compute_storage as storage};

const FW: u64 = 0xffff_fc10_0000_0000;
const COMMAND_FW: u64 = FW + 0x20000;
const COMMAND_GPU: u64 = 0x73_0000_3760;
const PREEMPTION_GPU: u64 = 0x71_0080_0000;
const CDM_GPU: u64 = 0x72_0000_0000;
const GUARD: u8 = 0xa5;

fn fw(offset: u64) -> FirmwareVa {
    FirmwareVa::new(FW + offset).unwrap()
}
fn command() -> Command {
    Command {
        context: 63,
        counter: 0x12345678,
        notifier: fw(0x40000),
        preemption: GpuRegion::new(PREEMPTION_GPU, 0x10000).unwrap(),
        cdm: GpuVa::new(CDM_GPU).unwrap(),
        // A one-dword empty/terminate stream: inclusive last == first.
        cdm_last: GpuVa::new(CDM_GPU).unwrap(),
        gpu_alias: GpuVa::new(COMMAND_GPU).unwrap(),
        microsequence: fw(0x60000),
        microsequence_size: seq::LENGTH as u32,
        stamp: fw(0x80000),
        fw_stamp: fw(0xa0000),
        stamp_value: 0xc1000100,
        timestamps: [fw(0xc0000), fw(0xe0000)],
        pool: fw(0x100000),
        tail_scratch: fw(0x120000),
        flush_stamps: true,
    }
}
fn sequence() -> Sequence {
    Sequence {
        command: Region::new(COMMAND_FW, cmd::SIZE).unwrap(),
        stats: Region::new(FW + 0x140000, 0x3748).unwrap(),
        notifier: Region::new(FW + 0x40000, 0xb0).unwrap(),
        queue: fw(0x160000),
        context: 63,
        generation: 7,
        pool: fw(0x100000),
        scratch: [
            fw(0x180000),
            fw(0x1a0000),
            fw(0x1c0000),
            fw(0x1e0000),
            fw(0x200000),
        ],
        fw_stamp: fw(0xa0000),
        stamp_value: 0xc1000100,
    }
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[test]
fn every_short_output_is_rejected_without_mutation() {
    let command = command();
    for length in 0..cmd::SIZE {
        let mut out = vec![GUARD; length];
        assert_eq!(command.encode(&mut out), Err(m3_queue_layout::Error::Size));
        assert!(out.iter().all(|&b| b == GUARD), "command length {length}");
    }
    let sequence = sequence();
    for length in 0..seq::STORAGE {
        let mut out = vec![GUARD; length];
        assert_eq!(sequence.encode(&mut out), Err(m3_init_layout::Error::Size));
        assert!(out.iter().all(|&b| b == GUARD), "sequence length {length}");
    }
}

#[test]
fn command_validation_is_atomic_and_accepts_boundary_values() {
    use m3_queue_layout::Error;
    for (case, expected) in [
        (0, Error::Address),
        (1, Error::Address),
        (2, Error::Size),
        (3, Error::Size),
        (4, Error::Size),
        (5, Error::Address),
    ] {
        let mut value = command();
        match case {
            0 => value.context = 64,
            1 => value.cdm_last = GpuVa::new(CDM_GPU - 4).unwrap(),
            2 => value.preemption = GpuRegion::new(PREEMPTION_GPU, 0x149f).unwrap(),
            3 => value.microsequence_size = 0,
            4 => value.microsequence_size = 4097,
            5 => value.gpu_alias = GpuVa::new((1 << 42) - 4).unwrap(),
            _ => unreachable!(),
        }
        let mut out = [GUARD; cmd::SIZE + 8];
        assert_eq!(value.encode(&mut out), Err(expected), "case {case}");
        assert_eq!(out, [GUARD; cmd::SIZE + 8]);
    }
    let mut value = command();
    value.preemption = GpuRegion::new(PREEMPTION_GPU, 0x14a0).unwrap();
    // Highest alias whose inclusive final aligned dword is still below 2^42.
    value.gpu_alias = GpuVa::new((1 << 42) - 4 - (cmd::SIZE - 3) as u64).unwrap();
    for length in [1, 4096] {
        value.microsequence_size = length;
        value.encode(&mut [GUARD; cmd::SIZE]).unwrap();
    }
}

#[test]
fn gpu_byte_addresses_reject_wrong_address_spaces_and_alignment() {
    for address in [0, 1, CDM_GPU + 2, 1 << 42, FW] {
        assert!(matches!(
            GpuVa::new(address),
            Err(m3_queue_layout::Error::Address)
        ));
    }
    for address in [4, COMMAND_GPU, (1 << 42) - 4] {
        assert_eq!(GpuVa::new(address).unwrap().get(), address);
    }
}

#[test]
fn command_register_alias_and_inclusive_cdm_end_are_packed_correctly() {
    let value = command();
    let mut out = [GUARD; cmd::SIZE + 16];
    value.encode(&mut out).unwrap();
    assert_eq!(u32_at(&out, 0), 3);
    assert_eq!(u64_at(&out, cmd::COUNTER), u64::from(value.counter));
    assert_eq!(u32_at(&out, cmd::CONTEXT), 63);
    assert_eq!(u64_at(&out, 0x14), value.notifier.get());
    assert_eq!(u64_at(&out, 0x720), COMMAND_GPU + 0x20);
    assert_ne!(u64_at(&out, 0x720), COMMAND_FW + 0x20);
    assert_eq!(
        u16::from_le_bytes(out[0x728..0x72a].try_into().unwrap()),
        22
    );
    assert_eq!(
        u16::from_le_bytes(out[0x72a..0x72c].try_into().unwrap()),
        264
    );
    // Decode the actual packed register records, including unaligned u64 data.
    for (index, register, expected) in [
        (0, 0x1a510, PREEMPTION_GPU),
        (1, 0x1a420, CDM_GPU),
        (2, 0x1a4d0, PREEMPTION_GPU + 0x1480),
        (5, 0x1a4e8, PREEMPTION_GPU + 0x1498),
    ] {
        let offset = cmd::REGISTERS + index * cmd::REGISTER_STRIDE;
        assert_eq!(u32_at(&out, offset), register);
        assert_eq!(u64_at(&out, offset + 4), expected);
    }
    assert_eq!(u64_at(&out, 0x794), PREEMPTION_GPU);
    assert_eq!(u64_at(&out, 0x79c), CDM_GPU); // not exclusive CDM_GPU + 4
    assert_eq!(
        u64_at(&out, 0x7c4),
        u64_at(&out, cmd::REGISTERS + 6 * 12 + 4)
    );
    assert_eq!(u64_at(&out, 0x760), value.microsequence.get());
    assert_eq!(u32_at(&out, 0x768), seq::LENGTH as u32);
    assert_eq!(u32_at(&out, cmd::FLUSH_STAMPS), 1);
    assert!(out[cmd::SIZE..].iter().all(|&b| b == GUARD));
    assert!(out[0x12c..0x720].iter().all(|&b| b == 0));
    let mut value = value;
    value.flush_stamps = false;
    value.encode(&mut out).unwrap();
    assert_eq!(u32_at(&out, cmd::FLUSH_STAMPS), 0);
}

#[test]
fn sequence_region_truncation_and_context_errors_are_atomic() {
    // Each command reference has a distinct extent; exercise all short regions
    // through the final flag reference, not just a single undersized fixture.
    for size in 1..0x884 {
        let mut value = sequence();
        value.command = Region::new(COMMAND_FW, size).unwrap();
        let mut out = [GUARD; seq::STORAGE + 8];
        assert_eq!(
            value.encode(&mut out),
            Err(m3_init_layout::Error::Bounds),
            "size {size}"
        );
        assert_eq!(out, [GUARD; seq::STORAGE + 8]);
    }
    for case in 0..3 {
        let mut value = sequence();
        let expected = match case {
            0 => {
                value.stats = Region::new(FW + 0x140000, 0x3747).unwrap();
                m3_init_layout::Error::Bounds
            }
            1 => {
                value.notifier = Region::new(FW + 0x40000, 0xaf).unwrap();
                m3_init_layout::Error::Bounds
            }
            _ => {
                value.context = 64;
                m3_init_layout::Error::Address
            }
        };
        let mut out = [GUARD; seq::STORAGE];
        assert_eq!(value.encode(&mut out), Err(expected));
        assert_eq!(out, [GUARD; seq::STORAGE]);
    }
    let mut value = sequence();
    value.command = Region::new(COMMAND_FW, 0x884).unwrap();
    value.encode(&mut [GUARD; seq::STORAGE]).unwrap();
}

#[test]
fn sequence_uses_firmware_field_addresses_not_gpu_aliases_or_pointees() {
    let command = command();
    let sequence = sequence();
    let mut command_bytes = [GUARD; cmd::SIZE];
    let mut out = [GUARD; seq::STORAGE + 16];
    command.encode(&mut command_bytes).unwrap();
    sequence.encode(&mut out).unwrap();
    assert_eq!(u64_at(&out, seq::START + 0x14), COMMAND_FW + 0x20);
    assert_ne!(u64_at(&out, seq::START + 0x14), COMMAND_GPU + 0x20);
    assert_eq!(u64_at(&out, seq::START + 0x44), COMMAND_FW + 0x76c);
    assert_eq!(u64_at(&out, seq::START + 0x1a4), COMMAND_FW + 0x880);
    assert_eq!(
        u64_at(&out, seq::START + 0x1b4),
        sequence.notifier.at(0xa8, 8).unwrap()
    );
    assert_eq!(u64_at(&out, seq::FINALIZE + 0x18), COMMAND_FW + 0x76c);
    // Finalize references the pool pointer FIELD, not the pool buffer itself.
    assert_eq!(u64_at(&out, seq::FINALIZE + 0x58), COMMAND_FW + 0x83e);
    assert_eq!(u64_at(&command_bytes, 0x83e), sequence.pool.get());
    assert_ne!(u64_at(&out, seq::FINALIZE + 0x58), sequence.pool.get());
    assert_eq!(u64_at(&out, seq::FINALIZE + 0x65), COMMAND_FW + 0x880);
    assert_eq!(u64_at(&out, seq::FINALIZE + 0x6d), COMMAND_FW + 0x860);
    for (index, offset) in [0x174, 0x184, 0x18c, 0x194, 0x19c].into_iter().enumerate() {
        assert_eq!(u64_at(&out, offset), sequence.scratch[index].get());
    }
    for start in [seq::TIMESTAMP_START, seq::TIMESTAMP_END] {
        assert_eq!(u64_at(&out, start + 4), COMMAND_FW + 0x810);
        assert_eq!(u64_at(&out, start + 0xc), COMMAND_FW + 0x818);
        assert_eq!(u64_at(&out, start + 0x2c), COMMAND_FW + 0x868);
    }
    assert_eq!(
        u64_at(&out, seq::TIMESTAMP_START + 0x14),
        COMMAND_FW + 0x818
    );
    assert_eq!(u64_at(&out, seq::TIMESTAMP_END + 0x14), COMMAND_FW + 0x820);
    assert_eq!(u64_at(&command_bytes, 0x818), command.timestamps[0].get());
    assert_eq!(u64_at(&command_bytes, 0x820), command.timestamps[1].get());
    assert_eq!(
        u64_at(&out, seq::FINALIZE + 0x28),
        u64_at(&command_bytes, cmd::META + 8)
    );
    assert_eq!(
        u32_at(&out, seq::STAMP_VALUE),
        u32_at(&command_bytes, cmd::STAMP_VALUE)
    );
    assert!(out[seq::LENGTH..seq::STORAGE].iter().all(|&b| b == 0));
    assert!(out[seq::STORAGE..].iter().all(|&b| b == GUARD));
}

#[test]
fn empty_sequence_opcodes_restart_and_attachment_fields_are_bounded() {
    let value = sequence();
    let mut out = [GUARD; seq::STORAGE];
    value.encode(&mut out).unwrap();
    for (offset, opcode) in [
        (seq::START, 0x0b),
        (seq::TIMESTAMP_START, 0x80000003),
        (seq::WAIT, 1),
        (seq::TIMESTAMP_END, 3),
        (seq::FINALIZE, 0x0c),
        (seq::END, 0x40000002),
    ] {
        assert_eq!(u32_at(&out, offset), opcode);
    }
    assert_eq!(u32_at(&out, seq::GENERATION), value.generation);
    assert!(out[seq::ATTACHMENTS..seq::START + 0x154]
        .iter()
        .all(|&b| b == 0));
    // This is a byte, immediately followed by an unaligned pointer at +0x65.
    assert_eq!(out[seq::HAS_ATTACHMENTS], 0);
    let restart = u32_at(&out, seq::FINALIZE + 0x60) as i32;
    assert_eq!(seq::FINALIZE as i32 + restart, seq::START as i32);
}

#[test]
fn compute_storage_keeps_all_live_slots_disjoint_and_shared_state_stable() {
    for index in 0..storage::COUNT {
        let allocation = storage::allocation(index).unwrap();
        let batch = storage::batch_allocation(index).unwrap();
        let mut previous_end = 0;
        for slot in 0..storage::SLOTS {
            let offset = storage::slot_offset(index, slot).unwrap();
            assert!(offset + allocation.size <= batch.size);
            if storage::RESET.contains(&index) {
                assert_eq!(offset & 0x3fff, 0);
                assert!(offset >= previous_end, "owner {index}, slot {slot}");
                previous_end = offset + allocation.size;
            } else {
                assert_eq!(offset, 0);
                assert_eq!(batch.size, allocation.size);
            }
        }
        assert!(storage::slot_offset(index, storage::SLOTS).is_err());
    }
    assert!(storage::allocation(storage::COUNT).is_err());
    assert!(storage::batch_allocation(usize::MAX).is_err());
    assert!(storage::slot_offset(storage::COUNT, 0).is_err());
    assert_eq!(
        storage::allocation(storage::COMMAND).unwrap().size,
        cmd::SIZE
    );
    assert_eq!(
        storage::allocation(storage::MICROSEQUENCE).unwrap().size,
        seq::STORAGE
    );
}
