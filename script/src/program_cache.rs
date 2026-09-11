//! One bounded root-program parse result per thread; no transaction or VM state.
//! Loading, mapping receipts and local time policy remain per attempt.
use ckb_types::{packed::Byte32, prelude::*};
use ckb_vm::{
    Bytes, Error,
    elf::{ProgramMetadata, parse_elf},
};
use std::{cell::RefCell, sync::Arc};

// Bound actual retained Vec storage, not only the number of populated actions.
const MAX_CACHED_ACTION_CAPACITY: usize = 64;

struct CachedProgram {
    data_hash: [u8; 32],
    version: u32,
    metadata: Arc<ProgramMetadata>,
}

thread_local! {
    static ROOT_PROGRAM: RefCell<Option<CachedProgram>> = const { RefCell::new(None) };
}

pub(crate) fn root_program_metadata(
    data_hash: &Byte32,
    version: u32,
    program: &Bytes,
) -> Result<Arc<ProgramMetadata>, Error> {
    let data_hash = data_hash.unpack();
    let cached = ROOT_PROGRAM
        .try_with(|cache| {
            cache
                .borrow()
                .as_ref()
                .filter(|cached| cached.data_hash == data_hash && cached.version == version)
                .map(|cached| Arc::clone(&cached.metadata))
        })
        .ok()
        .flatten();
    if let Some(metadata) = cached {
        return Ok(metadata);
    }
    // Parsing runs outside the borrow. A miss, oversized result or unavailable
    // thread-local slot follows the same parser and loader as an uncached call.
    let metadata = Arc::new(parse_elf::<u64>(program, version)?);
    if metadata.actions.capacity() <= MAX_CACHED_ACTION_CAPACITY {
        let _ = ROOT_PROGRAM.try_with(|cache| {
            *cache.borrow_mut() = Some(CachedProgram {
                data_hash,
                version,
                metadata: Arc::clone(&metadata),
            });
        });
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InitialProgramLoadReceipt;
    use ckb_types::packed::CellOutput;

    #[test]
    fn cache_separates_content_and_version_and_keeps_receipts_per_attempt() {
        let program = Bytes::from_static(include_bytes!("../testdata/always_success"));
        let hash = CellOutput::calc_data_hash(&program);
        let first = root_program_metadata(&hash, 0, &program).unwrap();
        let hit = root_program_metadata(&hash, 0, &program).unwrap();
        assert!(Arc::ptr_eq(&first, &hit));
        assert_eq!(*first, parse_elf::<u64>(&program, 0).unwrap());
        let receipt = InitialProgramLoadReceipt::from_metadata(&hit).unwrap();
        assert!(
            !crate::InitialProgramLoadLimit::new(1)
                .unwrap()
                .admits(receipt)
        );
        assert!(
            crate::InitialProgramLoadLimit::new(receipt.mapped_bytes())
                .unwrap()
                .admits(receipt)
        );

        let version = root_program_metadata(&hash, 1, &program).unwrap();
        assert!(!Arc::ptr_eq(&first, &version));
        assert_eq!(*version, parse_elf::<u64>(&program, 1).unwrap());
        let mut changed = program.to_vec();
        changed.push(0);
        let changed = Bytes::from(changed);
        let changed_hash = CellOutput::calc_data_hash(&changed);
        let other = root_program_metadata(&changed_hash, 1, &changed).unwrap();
        assert!(!Arc::ptr_eq(&version, &other));
        assert_eq!(*other, parse_elf::<u64>(&changed, 1).unwrap());

        let malformed = Bytes::from_static(b"not an ELF");
        let malformed_hash = CellOutput::calc_data_hash(&malformed);
        assert_eq!(
            root_program_metadata(&malformed_hash, 1, &malformed).unwrap_err(),
            parse_elf::<u64>(&malformed, 1).unwrap_err(),
        );
        assert!(Arc::ptr_eq(
            &other,
            &root_program_metadata(&changed_hash, 1, &changed).unwrap()
        ));
    }

    #[test]
    fn type_script_identity_uses_the_selected_program_data_hash() {
        use crate::{TxVerifyEnv, types::TxData};
        use ckb_chain_spec::consensus::ConsensusBuilder;
        use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
        use ckb_types::{
            core::{
                HeaderBuilder, ScriptHashType, TransactionBuilder,
                cell::{CellMetaBuilder, ResolvedTransaction},
            },
            packed::{OutPoint, Script},
        };
        #[derive(Clone)]
        struct Loader;
        impl CellDataProvider for Loader {
            fn get_cell_data(&self, _: &OutPoint) -> Option<Bytes> {
                None
            }
            fn get_cell_data_hash(&self, _: &OutPoint) -> Option<Byte32> {
                None
            }
        }
        impl HeaderProvider for Loader {
            fn get_header(&self, _: &Byte32) -> Option<ckb_types::core::HeaderView> {
                None
            }
        }
        impl ExtensionProvider for Loader {
            fn get_block_extension(&self, _: &Byte32) -> Option<ckb_types::packed::Bytes> {
                None
            }
        }
        let type_script = Script::default();
        let root = Script::new_builder()
            .hash_type(ScriptHashType::Type)
            .code_hash(type_script.calc_script_hash())
            .build();
        let program = Bytes::from_static(include_bytes!("../testdata/always_success"));
        let mut different = program.to_vec();
        different.push(0);
        let mut results = Vec::new();
        for data in [program, Bytes::from(different)] {
            let data_hash = CellOutput::calc_data_hash(&data);
            let output = CellOutput::new_builder()
                .type_(Some(type_script.clone()))
                .build();
            let cell = CellMetaBuilder::from_cell_output(output, data.clone()).build();
            let tx_data = TxData::new(
                Arc::new(ResolvedTransaction {
                    transaction: TransactionBuilder::default().build(),
                    resolved_cell_deps: vec![cell],
                    resolved_inputs: vec![],
                    resolved_dep_groups: vec![],
                }),
                Loader,
                Arc::new(ConsensusBuilder::default().build()),
                Arc::new(TxVerifyEnv::new_commit(&HeaderBuilder::default().build())),
            );
            let program_id = crate::types::DataPieceId::CellDep(0);
            let selected_hash = tx_data
                .info
                .extract_program_data_hash(&root, &program_id)
                .unwrap();
            assert_eq!(selected_hash, &data_hash);
            assert_ne!(selected_hash, &root.code_hash());
            assert!(
                tx_data
                    .info
                    .extract_program_data_hash(&Script::default(), &program_id)
                    .is_none()
            );
            assert!(
                tx_data
                    .info
                    .extract_program_data_hash(&root, &crate::types::DataPieceId::CellDep(1))
                    .is_none()
            );
            results.push(
                root_program_metadata(
                    selected_hash,
                    tx_data.select_version(&root).unwrap().vm_version(),
                    &data,
                )
                .unwrap(),
            );
            // A missing repeat selection must still parse/load the already
            // selected data piece without introducing a new script error.
            let mut sg = crate::types::SgData::new(
                &tx_data,
                &crate::types::ScriptGroup::from_lock_script(&root),
            )
            .unwrap();
            let version = sg.sg_info.script_version.vm_version();
            Arc::make_mut(&mut sg.sg_info).script_group.script = Script::default();
            let expected_receipt = InitialProgramLoadReceipt::from_metadata(
                &parse_elf::<u64>(&data, version).unwrap(),
            );
            let mut scheduler: crate::Scheduler<Loader, (), crate::types::Machine> =
                crate::Scheduler::new(sg, |_, _, _, _| Vec::new(), ());
            assert_eq!(
                scheduler.prepare_root_program_load().unwrap(),
                expected_receipt
            );
        }
        assert!(!Arc::ptr_eq(&results[0], &results[1]));
    }

    #[test]
    fn thread_exit_releases_the_slot_without_invalidating_prepared_metadata() {
        let program = Bytes::from_static(include_bytes!("../testdata/always_success"));
        let hash = CellOutput::calc_data_hash(&program);
        let expected = parse_elf::<u64>(&program, 0).unwrap();
        let metadata =
            std::thread::spawn(move || root_program_metadata(&hash, 0, &program).unwrap())
                .join()
                .unwrap();
        assert_eq!(*metadata, expected);
        let retired = Arc::downgrade(&metadata);
        drop(metadata);
        assert!(retired.upgrade().is_none());
    }

    #[test]
    fn oversized_metadata_bypasses_the_slot() {
        let mut program = include_bytes!("../testdata/always_success").to_vec();
        let phoff = u64::from_le_bytes(program[32..40].try_into().unwrap()) as usize;
        let phentsize = u16::from_le_bytes(program[54..56].try_into().unwrap()) as usize;
        let phnum = u16::from_le_bytes(program[56..58].try_into().unwrap()) as usize;
        let header = program[phoff..]
            .chunks_exact(phentsize)
            .take(phnum)
            .find(|header| u32::from_le_bytes(header[..4].try_into().unwrap()) == 1)
            .unwrap()
            .to_vec();
        let new_phoff = program.len() as u64;
        let count = MAX_CACHED_ACTION_CAPACITY + 1;
        for _ in 0..count {
            program.extend_from_slice(&header);
        }
        program[32..40].copy_from_slice(&new_phoff.to_le_bytes());
        program[56..58].copy_from_slice(&(count as u16).to_le_bytes());
        let program = Bytes::from(program);
        let hash = CellOutput::calc_data_hash(&program);
        let first = root_program_metadata(&hash, 1, &program).unwrap();
        assert_eq!(first.actions.len(), count);
        let next = root_program_metadata(&hash, 1, &program).unwrap();
        assert_eq!(*first, *next);
        assert!(!Arc::ptr_eq(&first, &next));
    }
}
