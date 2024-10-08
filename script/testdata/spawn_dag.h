// Generated by Molecule 0.8.0

#define MOLECULEC_VERSION 8000
#define MOLECULE_API_VERSION_MIN 7000

#include "molecule_reader.h"
#include "molecule_builder.h"

#ifndef SPAWN_DAG_H
#define SPAWN_DAG_H

#ifdef __cplusplus
extern "C" {
#endif /* __cplusplus */

#ifndef MOLECULE_API_DECORATOR
#define __DEFINE_MOLECULE_API_DECORATOR_SPAWN_DAG
#define MOLECULE_API_DECORATOR
#endif /* MOLECULE_API_DECORATOR */

/*
 * Reader APIs
 */

#define                                 MolReader_VmIndex_verify(s, c)                  mol_verify_fixed_size(s, 8)
#define                                 MolReader_VmIndex_get_nth0(s)                   mol_slice_by_offset(s, 0, 1)
#define                                 MolReader_VmIndex_get_nth1(s)                   mol_slice_by_offset(s, 1, 1)
#define                                 MolReader_VmIndex_get_nth2(s)                   mol_slice_by_offset(s, 2, 1)
#define                                 MolReader_VmIndex_get_nth3(s)                   mol_slice_by_offset(s, 3, 1)
#define                                 MolReader_VmIndex_get_nth4(s)                   mol_slice_by_offset(s, 4, 1)
#define                                 MolReader_VmIndex_get_nth5(s)                   mol_slice_by_offset(s, 5, 1)
#define                                 MolReader_VmIndex_get_nth6(s)                   mol_slice_by_offset(s, 6, 1)
#define                                 MolReader_VmIndex_get_nth7(s)                   mol_slice_by_offset(s, 7, 1)
#define                                 MolReader_FdIndex_verify(s, c)                  mol_verify_fixed_size(s, 8)
#define                                 MolReader_FdIndex_get_nth0(s)                   mol_slice_by_offset(s, 0, 1)
#define                                 MolReader_FdIndex_get_nth1(s)                   mol_slice_by_offset(s, 1, 1)
#define                                 MolReader_FdIndex_get_nth2(s)                   mol_slice_by_offset(s, 2, 1)
#define                                 MolReader_FdIndex_get_nth3(s)                   mol_slice_by_offset(s, 3, 1)
#define                                 MolReader_FdIndex_get_nth4(s)                   mol_slice_by_offset(s, 4, 1)
#define                                 MolReader_FdIndex_get_nth5(s)                   mol_slice_by_offset(s, 5, 1)
#define                                 MolReader_FdIndex_get_nth6(s)                   mol_slice_by_offset(s, 6, 1)
#define                                 MolReader_FdIndex_get_nth7(s)                   mol_slice_by_offset(s, 7, 1)
#define                                 MolReader_FdIndices_verify(s, c)                mol_fixvec_verify(s, 8)
#define                                 MolReader_FdIndices_length(s)                   mol_fixvec_length(s)
#define                                 MolReader_FdIndices_get(s, i)                   mol_fixvec_slice_by_index(s, 8, i)
#define                                 MolReader_Bytes_verify(s, c)                    mol_fixvec_verify(s, 1)
#define                                 MolReader_Bytes_length(s)                       mol_fixvec_length(s)
#define                                 MolReader_Bytes_get(s, i)                       mol_fixvec_slice_by_index(s, 1, i)
#define                                 MolReader_Bytes_raw_bytes(s)                    mol_fixvec_slice_raw_bytes(s)
MOLECULE_API_DECORATOR  mol_errno       MolReader_Pipe_verify                           (const mol_seg_t*, bool);
#define                                 MolReader_Pipe_actual_field_count(s)            mol_table_actual_field_count(s)
#define                                 MolReader_Pipe_has_extra_fields(s)              mol_table_has_extra_fields(s, 3)
#define                                 MolReader_Pipe_get_vm(s)                        mol_table_slice_by_index(s, 0)
#define                                 MolReader_Pipe_get_read_fd(s)                   mol_table_slice_by_index(s, 1)
#define                                 MolReader_Pipe_get_write_fd(s)                  mol_table_slice_by_index(s, 2)
MOLECULE_API_DECORATOR  mol_errno       MolReader_Pipes_verify                          (const mol_seg_t*, bool);
#define                                 MolReader_Pipes_length(s)                       mol_dynvec_length(s)
#define                                 MolReader_Pipes_get(s, i)                       mol_dynvec_slice_by_index(s, i)
MOLECULE_API_DECORATOR  mol_errno       MolReader_Write_verify                          (const mol_seg_t*, bool);
#define                                 MolReader_Write_actual_field_count(s)           mol_table_actual_field_count(s)
#define                                 MolReader_Write_has_extra_fields(s)             mol_table_has_extra_fields(s, 5)
#define                                 MolReader_Write_get_from(s)                     mol_table_slice_by_index(s, 0)
#define                                 MolReader_Write_get_from_fd(s)                  mol_table_slice_by_index(s, 1)
#define                                 MolReader_Write_get_to(s)                       mol_table_slice_by_index(s, 2)
#define                                 MolReader_Write_get_to_fd(s)                    mol_table_slice_by_index(s, 3)
#define                                 MolReader_Write_get_data(s)                     mol_table_slice_by_index(s, 4)
MOLECULE_API_DECORATOR  mol_errno       MolReader_Writes_verify                         (const mol_seg_t*, bool);
#define                                 MolReader_Writes_length(s)                      mol_dynvec_length(s)
#define                                 MolReader_Writes_get(s, i)                      mol_dynvec_slice_by_index(s, i)
MOLECULE_API_DECORATOR  mol_errno       MolReader_Spawn_verify                          (const mol_seg_t*, bool);
#define                                 MolReader_Spawn_actual_field_count(s)           mol_table_actual_field_count(s)
#define                                 MolReader_Spawn_has_extra_fields(s)             mol_table_has_extra_fields(s, 3)
#define                                 MolReader_Spawn_get_from(s)                     mol_table_slice_by_index(s, 0)
#define                                 MolReader_Spawn_get_child(s)                    mol_table_slice_by_index(s, 1)
#define                                 MolReader_Spawn_get_fds(s)                      mol_table_slice_by_index(s, 2)
MOLECULE_API_DECORATOR  mol_errno       MolReader_Spawns_verify                         (const mol_seg_t*, bool);
#define                                 MolReader_Spawns_length(s)                      mol_dynvec_length(s)
#define                                 MolReader_Spawns_get(s, i)                      mol_dynvec_slice_by_index(s, i)
MOLECULE_API_DECORATOR  mol_errno       MolReader_Data_verify                           (const mol_seg_t*, bool);
#define                                 MolReader_Data_actual_field_count(s)            mol_table_actual_field_count(s)
#define                                 MolReader_Data_has_extra_fields(s)              mol_table_has_extra_fields(s, 3)
#define                                 MolReader_Data_get_spawns(s)                    mol_table_slice_by_index(s, 0)
#define                                 MolReader_Data_get_pipes(s)                     mol_table_slice_by_index(s, 1)
#define                                 MolReader_Data_get_writes(s)                    mol_table_slice_by_index(s, 2)

/*
 * Builder APIs
 */

#define                                 MolBuilder_VmIndex_init(b)                      mol_builder_initialize_fixed_size(b, 8)
#define                                 MolBuilder_VmIndex_set_nth0(b, p)               mol_builder_set_byte_by_offset(b, 0, p)
#define                                 MolBuilder_VmIndex_set_nth1(b, p)               mol_builder_set_byte_by_offset(b, 1, p)
#define                                 MolBuilder_VmIndex_set_nth2(b, p)               mol_builder_set_byte_by_offset(b, 2, p)
#define                                 MolBuilder_VmIndex_set_nth3(b, p)               mol_builder_set_byte_by_offset(b, 3, p)
#define                                 MolBuilder_VmIndex_set_nth4(b, p)               mol_builder_set_byte_by_offset(b, 4, p)
#define                                 MolBuilder_VmIndex_set_nth5(b, p)               mol_builder_set_byte_by_offset(b, 5, p)
#define                                 MolBuilder_VmIndex_set_nth6(b, p)               mol_builder_set_byte_by_offset(b, 6, p)
#define                                 MolBuilder_VmIndex_set_nth7(b, p)               mol_builder_set_byte_by_offset(b, 7, p)
#define                                 MolBuilder_VmIndex_build(b)                     mol_builder_finalize_simple(b)
#define                                 MolBuilder_VmIndex_clear(b)                     mol_builder_discard(b)
#define                                 MolBuilder_FdIndex_init(b)                      mol_builder_initialize_fixed_size(b, 8)
#define                                 MolBuilder_FdIndex_set_nth0(b, p)               mol_builder_set_byte_by_offset(b, 0, p)
#define                                 MolBuilder_FdIndex_set_nth1(b, p)               mol_builder_set_byte_by_offset(b, 1, p)
#define                                 MolBuilder_FdIndex_set_nth2(b, p)               mol_builder_set_byte_by_offset(b, 2, p)
#define                                 MolBuilder_FdIndex_set_nth3(b, p)               mol_builder_set_byte_by_offset(b, 3, p)
#define                                 MolBuilder_FdIndex_set_nth4(b, p)               mol_builder_set_byte_by_offset(b, 4, p)
#define                                 MolBuilder_FdIndex_set_nth5(b, p)               mol_builder_set_byte_by_offset(b, 5, p)
#define                                 MolBuilder_FdIndex_set_nth6(b, p)               mol_builder_set_byte_by_offset(b, 6, p)
#define                                 MolBuilder_FdIndex_set_nth7(b, p)               mol_builder_set_byte_by_offset(b, 7, p)
#define                                 MolBuilder_FdIndex_build(b)                     mol_builder_finalize_simple(b)
#define                                 MolBuilder_FdIndex_clear(b)                     mol_builder_discard(b)
#define                                 MolBuilder_FdIndices_init(b)                    mol_fixvec_builder_initialize(b, 128)
#define                                 MolBuilder_FdIndices_push(b, p)                 mol_fixvec_builder_push(b, p, 8)
#define                                 MolBuilder_FdIndices_build(b)                   mol_fixvec_builder_finalize(b)
#define                                 MolBuilder_FdIndices_clear(b)                   mol_builder_discard(b)
#define                                 MolBuilder_Bytes_init(b)                        mol_fixvec_builder_initialize(b, 16)
#define                                 MolBuilder_Bytes_push(b, p)                     mol_fixvec_builder_push_byte(b, p)
#define                                 MolBuilder_Bytes_build(b)                       mol_fixvec_builder_finalize(b)
#define                                 MolBuilder_Bytes_clear(b)                       mol_builder_discard(b)
#define                                 MolBuilder_Pipe_init(b)                         mol_table_builder_initialize(b, 256, 3)
#define                                 MolBuilder_Pipe_set_vm(b, p, l)                 mol_table_builder_add(b, 0, p, l)
#define                                 MolBuilder_Pipe_set_read_fd(b, p, l)            mol_table_builder_add(b, 1, p, l)
#define                                 MolBuilder_Pipe_set_write_fd(b, p, l)           mol_table_builder_add(b, 2, p, l)
MOLECULE_API_DECORATOR  mol_seg_res_t   MolBuilder_Pipe_build                           (mol_builder_t);
#define                                 MolBuilder_Pipe_clear(b)                        mol_builder_discard(b)
#define                                 MolBuilder_Pipes_init(b)                        mol_builder_initialize_with_capacity(b, 1024, 64)
#define                                 MolBuilder_Pipes_push(b, p, l)                  mol_dynvec_builder_push(b, p, l)
#define                                 MolBuilder_Pipes_build(b)                       mol_dynvec_builder_finalize(b)
#define                                 MolBuilder_Pipes_clear(b)                       mol_builder_discard(b)
#define                                 MolBuilder_Write_init(b)                        mol_table_builder_initialize(b, 256, 5)
#define                                 MolBuilder_Write_set_from(b, p, l)              mol_table_builder_add(b, 0, p, l)
#define                                 MolBuilder_Write_set_from_fd(b, p, l)           mol_table_builder_add(b, 1, p, l)
#define                                 MolBuilder_Write_set_to(b, p, l)                mol_table_builder_add(b, 2, p, l)
#define                                 MolBuilder_Write_set_to_fd(b, p, l)             mol_table_builder_add(b, 3, p, l)
#define                                 MolBuilder_Write_set_data(b, p, l)              mol_table_builder_add(b, 4, p, l)
MOLECULE_API_DECORATOR  mol_seg_res_t   MolBuilder_Write_build                          (mol_builder_t);
#define                                 MolBuilder_Write_clear(b)                       mol_builder_discard(b)
#define                                 MolBuilder_Writes_init(b)                       mol_builder_initialize_with_capacity(b, 1024, 64)
#define                                 MolBuilder_Writes_push(b, p, l)                 mol_dynvec_builder_push(b, p, l)
#define                                 MolBuilder_Writes_build(b)                      mol_dynvec_builder_finalize(b)
#define                                 MolBuilder_Writes_clear(b)                      mol_builder_discard(b)
#define                                 MolBuilder_Spawn_init(b)                        mol_table_builder_initialize(b, 256, 3)
#define                                 MolBuilder_Spawn_set_from(b, p, l)              mol_table_builder_add(b, 0, p, l)
#define                                 MolBuilder_Spawn_set_child(b, p, l)             mol_table_builder_add(b, 1, p, l)
#define                                 MolBuilder_Spawn_set_fds(b, p, l)               mol_table_builder_add(b, 2, p, l)
MOLECULE_API_DECORATOR  mol_seg_res_t   MolBuilder_Spawn_build                          (mol_builder_t);
#define                                 MolBuilder_Spawn_clear(b)                       mol_builder_discard(b)
#define                                 MolBuilder_Spawns_init(b)                       mol_builder_initialize_with_capacity(b, 1024, 64)
#define                                 MolBuilder_Spawns_push(b, p, l)                 mol_dynvec_builder_push(b, p, l)
#define                                 MolBuilder_Spawns_build(b)                      mol_dynvec_builder_finalize(b)
#define                                 MolBuilder_Spawns_clear(b)                      mol_builder_discard(b)
#define                                 MolBuilder_Data_init(b)                         mol_table_builder_initialize(b, 128, 3)
#define                                 MolBuilder_Data_set_spawns(b, p, l)             mol_table_builder_add(b, 0, p, l)
#define                                 MolBuilder_Data_set_pipes(b, p, l)              mol_table_builder_add(b, 1, p, l)
#define                                 MolBuilder_Data_set_writes(b, p, l)             mol_table_builder_add(b, 2, p, l)
MOLECULE_API_DECORATOR  mol_seg_res_t   MolBuilder_Data_build                           (mol_builder_t);
#define                                 MolBuilder_Data_clear(b)                        mol_builder_discard(b)

/*
 * Default Value
 */

#define ____ 0x00

MOLECULE_API_DECORATOR const uint8_t MolDefault_VmIndex[8]       =  {
    ____, ____, ____, ____, ____, ____, ____, ____,
};
MOLECULE_API_DECORATOR const uint8_t MolDefault_FdIndex[8]       =  {
    ____, ____, ____, ____, ____, ____, ____, ____,
};
MOLECULE_API_DECORATOR const uint8_t MolDefault_FdIndices[4]     =  {____, ____, ____, ____};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Bytes[4]         =  {____, ____, ____, ____};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Pipe[40]         =  {
    0x28, ____, ____, ____, 0x10, ____, ____, ____, 0x18, ____, ____, ____,
    0x20, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
    ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
    ____, ____, ____, ____,
};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Pipes[4]         =  {0x04, ____, ____, ____};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Write[60]        =  {
    0x3c, ____, ____, ____, 0x18, ____, ____, ____, 0x20, ____, ____, ____,
    0x28, ____, ____, ____, 0x30, ____, ____, ____, 0x38, ____, ____, ____,
    ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
    ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
    ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Writes[4]        =  {0x04, ____, ____, ____};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Spawn[36]        =  {
    0x24, ____, ____, ____, 0x10, ____, ____, ____, 0x18, ____, ____, ____,
    0x20, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
    ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Spawns[4]        =  {0x04, ____, ____, ____};
MOLECULE_API_DECORATOR const uint8_t MolDefault_Data[28]         =  {
    0x1c, ____, ____, ____, 0x10, ____, ____, ____, 0x14, ____, ____, ____,
    0x18, ____, ____, ____, 0x04, ____, ____, ____, 0x04, ____, ____, ____,
    0x04, ____, ____, ____,
};

#undef ____

/*
 * Reader Functions
 */

MOLECULE_API_DECORATOR mol_errno MolReader_Pipe_verify (const mol_seg_t *input, bool compatible) {
    if (input->size < MOL_NUM_T_SIZE) {
        return MOL_ERR_HEADER;
    }
    uint8_t *ptr = input->ptr;
    mol_num_t total_size = mol_unpack_number(ptr);
    if (input->size != total_size) {
        return MOL_ERR_TOTAL_SIZE;
    }
    if (input->size < MOL_NUM_T_SIZE * 2) {
        return MOL_ERR_HEADER;
    }
    ptr += MOL_NUM_T_SIZE;
    mol_num_t offset = mol_unpack_number(ptr);
    if (offset % 4 > 0 || offset < MOL_NUM_T_SIZE*2) {
        return MOL_ERR_OFFSET;
    }
    mol_num_t field_count = offset / 4 - 1;
    if (field_count < 3) {
        return MOL_ERR_FIELD_COUNT;
    } else if (!compatible && field_count > 3) {
        return MOL_ERR_FIELD_COUNT;
    }
    if (input->size < MOL_NUM_T_SIZE*(field_count+1)){
        return MOL_ERR_HEADER;
    }
    mol_num_t offsets[field_count+1];
    offsets[0] = offset;
    for (mol_num_t i=1; i<field_count; i++) {
        ptr += MOL_NUM_T_SIZE;
        offsets[i] = mol_unpack_number(ptr);
        if (offsets[i-1] > offsets[i]) {
            return MOL_ERR_OFFSET;
        }
    }
    if (offsets[field_count-1] > total_size) {
        return MOL_ERR_OFFSET;
    }
    offsets[field_count] = total_size;
        mol_seg_t inner;
        mol_errno errno;
        inner.ptr = input->ptr + offsets[0];
        inner.size = offsets[1] - offsets[0];
        errno = MolReader_VmIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[1];
        inner.size = offsets[2] - offsets[1];
        errno = MolReader_FdIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[2];
        inner.size = offsets[3] - offsets[2];
        errno = MolReader_FdIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
    return MOL_OK;
}
MOLECULE_API_DECORATOR mol_errno MolReader_Pipes_verify (const mol_seg_t *input, bool compatible) {
    if (input->size < MOL_NUM_T_SIZE) {
        return MOL_ERR_HEADER;
    }
    uint8_t *ptr = input->ptr;
    mol_num_t total_size = mol_unpack_number(ptr);
    if (input->size != total_size) {
        return MOL_ERR_TOTAL_SIZE;
    }
    if (input->size == MOL_NUM_T_SIZE) {
        return MOL_OK;
    }
    if (input->size < MOL_NUM_T_SIZE * 2) {
        return MOL_ERR_HEADER;
    }
    ptr += MOL_NUM_T_SIZE;
    mol_num_t offset = mol_unpack_number(ptr);
    if (offset % 4 > 0 || offset < MOL_NUM_T_SIZE*2) {
        return MOL_ERR_OFFSET;
    }
    mol_num_t item_count = offset / 4 - 1;
    if (input->size < MOL_NUM_T_SIZE*(item_count+1)) {
        return MOL_ERR_HEADER;
    }
    mol_num_t end;
    for (mol_num_t i=1; i<item_count; i++) {
        ptr += MOL_NUM_T_SIZE;
        end = mol_unpack_number(ptr);
        if (offset > end) {
            return MOL_ERR_OFFSET;
        }
        mol_seg_t inner;
        inner.ptr = input->ptr + offset;
        inner.size = end - offset;
        mol_errno errno = MolReader_Pipe_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        offset = end;
    }
    if (offset > total_size) {
        return MOL_ERR_OFFSET;
    }
    mol_seg_t inner;
    inner.ptr = input->ptr + offset;
    inner.size = total_size - offset;
    return MolReader_Pipe_verify(&inner, compatible);
}
MOLECULE_API_DECORATOR mol_errno MolReader_Write_verify (const mol_seg_t *input, bool compatible) {
    if (input->size < MOL_NUM_T_SIZE) {
        return MOL_ERR_HEADER;
    }
    uint8_t *ptr = input->ptr;
    mol_num_t total_size = mol_unpack_number(ptr);
    if (input->size != total_size) {
        return MOL_ERR_TOTAL_SIZE;
    }
    if (input->size < MOL_NUM_T_SIZE * 2) {
        return MOL_ERR_HEADER;
    }
    ptr += MOL_NUM_T_SIZE;
    mol_num_t offset = mol_unpack_number(ptr);
    if (offset % 4 > 0 || offset < MOL_NUM_T_SIZE*2) {
        return MOL_ERR_OFFSET;
    }
    mol_num_t field_count = offset / 4 - 1;
    if (field_count < 5) {
        return MOL_ERR_FIELD_COUNT;
    } else if (!compatible && field_count > 5) {
        return MOL_ERR_FIELD_COUNT;
    }
    if (input->size < MOL_NUM_T_SIZE*(field_count+1)){
        return MOL_ERR_HEADER;
    }
    mol_num_t offsets[field_count+1];
    offsets[0] = offset;
    for (mol_num_t i=1; i<field_count; i++) {
        ptr += MOL_NUM_T_SIZE;
        offsets[i] = mol_unpack_number(ptr);
        if (offsets[i-1] > offsets[i]) {
            return MOL_ERR_OFFSET;
        }
    }
    if (offsets[field_count-1] > total_size) {
        return MOL_ERR_OFFSET;
    }
    offsets[field_count] = total_size;
        mol_seg_t inner;
        mol_errno errno;
        inner.ptr = input->ptr + offsets[0];
        inner.size = offsets[1] - offsets[0];
        errno = MolReader_VmIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[1];
        inner.size = offsets[2] - offsets[1];
        errno = MolReader_FdIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[2];
        inner.size = offsets[3] - offsets[2];
        errno = MolReader_VmIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[3];
        inner.size = offsets[4] - offsets[3];
        errno = MolReader_FdIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[4];
        inner.size = offsets[5] - offsets[4];
        errno = MolReader_Bytes_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
    return MOL_OK;
}
MOLECULE_API_DECORATOR mol_errno MolReader_Writes_verify (const mol_seg_t *input, bool compatible) {
    if (input->size < MOL_NUM_T_SIZE) {
        return MOL_ERR_HEADER;
    }
    uint8_t *ptr = input->ptr;
    mol_num_t total_size = mol_unpack_number(ptr);
    if (input->size != total_size) {
        return MOL_ERR_TOTAL_SIZE;
    }
    if (input->size == MOL_NUM_T_SIZE) {
        return MOL_OK;
    }
    if (input->size < MOL_NUM_T_SIZE * 2) {
        return MOL_ERR_HEADER;
    }
    ptr += MOL_NUM_T_SIZE;
    mol_num_t offset = mol_unpack_number(ptr);
    if (offset % 4 > 0 || offset < MOL_NUM_T_SIZE*2) {
        return MOL_ERR_OFFSET;
    }
    mol_num_t item_count = offset / 4 - 1;
    if (input->size < MOL_NUM_T_SIZE*(item_count+1)) {
        return MOL_ERR_HEADER;
    }
    mol_num_t end;
    for (mol_num_t i=1; i<item_count; i++) {
        ptr += MOL_NUM_T_SIZE;
        end = mol_unpack_number(ptr);
        if (offset > end) {
            return MOL_ERR_OFFSET;
        }
        mol_seg_t inner;
        inner.ptr = input->ptr + offset;
        inner.size = end - offset;
        mol_errno errno = MolReader_Write_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        offset = end;
    }
    if (offset > total_size) {
        return MOL_ERR_OFFSET;
    }
    mol_seg_t inner;
    inner.ptr = input->ptr + offset;
    inner.size = total_size - offset;
    return MolReader_Write_verify(&inner, compatible);
}
MOLECULE_API_DECORATOR mol_errno MolReader_Spawn_verify (const mol_seg_t *input, bool compatible) {
    if (input->size < MOL_NUM_T_SIZE) {
        return MOL_ERR_HEADER;
    }
    uint8_t *ptr = input->ptr;
    mol_num_t total_size = mol_unpack_number(ptr);
    if (input->size != total_size) {
        return MOL_ERR_TOTAL_SIZE;
    }
    if (input->size < MOL_NUM_T_SIZE * 2) {
        return MOL_ERR_HEADER;
    }
    ptr += MOL_NUM_T_SIZE;
    mol_num_t offset = mol_unpack_number(ptr);
    if (offset % 4 > 0 || offset < MOL_NUM_T_SIZE*2) {
        return MOL_ERR_OFFSET;
    }
    mol_num_t field_count = offset / 4 - 1;
    if (field_count < 3) {
        return MOL_ERR_FIELD_COUNT;
    } else if (!compatible && field_count > 3) {
        return MOL_ERR_FIELD_COUNT;
    }
    if (input->size < MOL_NUM_T_SIZE*(field_count+1)){
        return MOL_ERR_HEADER;
    }
    mol_num_t offsets[field_count+1];
    offsets[0] = offset;
    for (mol_num_t i=1; i<field_count; i++) {
        ptr += MOL_NUM_T_SIZE;
        offsets[i] = mol_unpack_number(ptr);
        if (offsets[i-1] > offsets[i]) {
            return MOL_ERR_OFFSET;
        }
    }
    if (offsets[field_count-1] > total_size) {
        return MOL_ERR_OFFSET;
    }
    offsets[field_count] = total_size;
        mol_seg_t inner;
        mol_errno errno;
        inner.ptr = input->ptr + offsets[0];
        inner.size = offsets[1] - offsets[0];
        errno = MolReader_VmIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[1];
        inner.size = offsets[2] - offsets[1];
        errno = MolReader_VmIndex_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[2];
        inner.size = offsets[3] - offsets[2];
        errno = MolReader_FdIndices_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
    return MOL_OK;
}
MOLECULE_API_DECORATOR mol_errno MolReader_Spawns_verify (const mol_seg_t *input, bool compatible) {
    if (input->size < MOL_NUM_T_SIZE) {
        return MOL_ERR_HEADER;
    }
    uint8_t *ptr = input->ptr;
    mol_num_t total_size = mol_unpack_number(ptr);
    if (input->size != total_size) {
        return MOL_ERR_TOTAL_SIZE;
    }
    if (input->size == MOL_NUM_T_SIZE) {
        return MOL_OK;
    }
    if (input->size < MOL_NUM_T_SIZE * 2) {
        return MOL_ERR_HEADER;
    }
    ptr += MOL_NUM_T_SIZE;
    mol_num_t offset = mol_unpack_number(ptr);
    if (offset % 4 > 0 || offset < MOL_NUM_T_SIZE*2) {
        return MOL_ERR_OFFSET;
    }
    mol_num_t item_count = offset / 4 - 1;
    if (input->size < MOL_NUM_T_SIZE*(item_count+1)) {
        return MOL_ERR_HEADER;
    }
    mol_num_t end;
    for (mol_num_t i=1; i<item_count; i++) {
        ptr += MOL_NUM_T_SIZE;
        end = mol_unpack_number(ptr);
        if (offset > end) {
            return MOL_ERR_OFFSET;
        }
        mol_seg_t inner;
        inner.ptr = input->ptr + offset;
        inner.size = end - offset;
        mol_errno errno = MolReader_Spawn_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        offset = end;
    }
    if (offset > total_size) {
        return MOL_ERR_OFFSET;
    }
    mol_seg_t inner;
    inner.ptr = input->ptr + offset;
    inner.size = total_size - offset;
    return MolReader_Spawn_verify(&inner, compatible);
}
MOLECULE_API_DECORATOR mol_errno MolReader_Data_verify (const mol_seg_t *input, bool compatible) {
    if (input->size < MOL_NUM_T_SIZE) {
        return MOL_ERR_HEADER;
    }
    uint8_t *ptr = input->ptr;
    mol_num_t total_size = mol_unpack_number(ptr);
    if (input->size != total_size) {
        return MOL_ERR_TOTAL_SIZE;
    }
    if (input->size < MOL_NUM_T_SIZE * 2) {
        return MOL_ERR_HEADER;
    }
    ptr += MOL_NUM_T_SIZE;
    mol_num_t offset = mol_unpack_number(ptr);
    if (offset % 4 > 0 || offset < MOL_NUM_T_SIZE*2) {
        return MOL_ERR_OFFSET;
    }
    mol_num_t field_count = offset / 4 - 1;
    if (field_count < 3) {
        return MOL_ERR_FIELD_COUNT;
    } else if (!compatible && field_count > 3) {
        return MOL_ERR_FIELD_COUNT;
    }
    if (input->size < MOL_NUM_T_SIZE*(field_count+1)){
        return MOL_ERR_HEADER;
    }
    mol_num_t offsets[field_count+1];
    offsets[0] = offset;
    for (mol_num_t i=1; i<field_count; i++) {
        ptr += MOL_NUM_T_SIZE;
        offsets[i] = mol_unpack_number(ptr);
        if (offsets[i-1] > offsets[i]) {
            return MOL_ERR_OFFSET;
        }
    }
    if (offsets[field_count-1] > total_size) {
        return MOL_ERR_OFFSET;
    }
    offsets[field_count] = total_size;
        mol_seg_t inner;
        mol_errno errno;
        inner.ptr = input->ptr + offsets[0];
        inner.size = offsets[1] - offsets[0];
        errno = MolReader_Spawns_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[1];
        inner.size = offsets[2] - offsets[1];
        errno = MolReader_Pipes_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
        inner.ptr = input->ptr + offsets[2];
        inner.size = offsets[3] - offsets[2];
        errno = MolReader_Writes_verify(&inner, compatible);
        if (errno != MOL_OK) {
            return MOL_ERR_DATA;
        }
    return MOL_OK;
}

/*
 * Builder Functions
 */

MOLECULE_API_DECORATOR mol_seg_res_t MolBuilder_Pipe_build (mol_builder_t builder) {
    mol_seg_res_t res;
    res.errno = MOL_OK;
    mol_num_t offset = 16;
    mol_num_t len;
    res.seg.size = offset;
    len = builder.number_ptr[1];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[3];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[5];
    res.seg.size += len == 0 ? 8 : len;
    res.seg.ptr = (uint8_t*)malloc(res.seg.size);
    uint8_t *dst = res.seg.ptr;
    mol_pack_number(dst, &res.seg.size);
    dst += MOL_NUM_T_SIZE;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[1];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[3];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[5];
    offset += len == 0 ? 8 : len;
    uint8_t *src = builder.data_ptr;
    len = builder.number_ptr[1];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_VmIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[0];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[3];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_FdIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[2];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[5];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_FdIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[4];
        memcpy(dst, src+of, len);
    }
    dst += len;
    mol_builder_discard(builder);
    return res;
}
MOLECULE_API_DECORATOR mol_seg_res_t MolBuilder_Write_build (mol_builder_t builder) {
    mol_seg_res_t res;
    res.errno = MOL_OK;
    mol_num_t offset = 24;
    mol_num_t len;
    res.seg.size = offset;
    len = builder.number_ptr[1];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[3];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[5];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[7];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[9];
    res.seg.size += len == 0 ? 4 : len;
    res.seg.ptr = (uint8_t*)malloc(res.seg.size);
    uint8_t *dst = res.seg.ptr;
    mol_pack_number(dst, &res.seg.size);
    dst += MOL_NUM_T_SIZE;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[1];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[3];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[5];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[7];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[9];
    offset += len == 0 ? 4 : len;
    uint8_t *src = builder.data_ptr;
    len = builder.number_ptr[1];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_VmIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[0];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[3];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_FdIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[2];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[5];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_VmIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[4];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[7];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_FdIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[6];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[9];
    if (len == 0) {
        len = 4;
        memcpy(dst, &MolDefault_Bytes, len);
    } else {
        mol_num_t of = builder.number_ptr[8];
        memcpy(dst, src+of, len);
    }
    dst += len;
    mol_builder_discard(builder);
    return res;
}
MOLECULE_API_DECORATOR mol_seg_res_t MolBuilder_Spawn_build (mol_builder_t builder) {
    mol_seg_res_t res;
    res.errno = MOL_OK;
    mol_num_t offset = 16;
    mol_num_t len;
    res.seg.size = offset;
    len = builder.number_ptr[1];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[3];
    res.seg.size += len == 0 ? 8 : len;
    len = builder.number_ptr[5];
    res.seg.size += len == 0 ? 4 : len;
    res.seg.ptr = (uint8_t*)malloc(res.seg.size);
    uint8_t *dst = res.seg.ptr;
    mol_pack_number(dst, &res.seg.size);
    dst += MOL_NUM_T_SIZE;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[1];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[3];
    offset += len == 0 ? 8 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[5];
    offset += len == 0 ? 4 : len;
    uint8_t *src = builder.data_ptr;
    len = builder.number_ptr[1];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_VmIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[0];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[3];
    if (len == 0) {
        len = 8;
        memcpy(dst, &MolDefault_VmIndex, len);
    } else {
        mol_num_t of = builder.number_ptr[2];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[5];
    if (len == 0) {
        len = 4;
        memcpy(dst, &MolDefault_FdIndices, len);
    } else {
        mol_num_t of = builder.number_ptr[4];
        memcpy(dst, src+of, len);
    }
    dst += len;
    mol_builder_discard(builder);
    return res;
}
MOLECULE_API_DECORATOR mol_seg_res_t MolBuilder_Data_build (mol_builder_t builder) {
    mol_seg_res_t res;
    res.errno = MOL_OK;
    mol_num_t offset = 16;
    mol_num_t len;
    res.seg.size = offset;
    len = builder.number_ptr[1];
    res.seg.size += len == 0 ? 4 : len;
    len = builder.number_ptr[3];
    res.seg.size += len == 0 ? 4 : len;
    len = builder.number_ptr[5];
    res.seg.size += len == 0 ? 4 : len;
    res.seg.ptr = (uint8_t*)malloc(res.seg.size);
    uint8_t *dst = res.seg.ptr;
    mol_pack_number(dst, &res.seg.size);
    dst += MOL_NUM_T_SIZE;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[1];
    offset += len == 0 ? 4 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[3];
    offset += len == 0 ? 4 : len;
    mol_pack_number(dst, &offset);
    dst += MOL_NUM_T_SIZE;
    len = builder.number_ptr[5];
    offset += len == 0 ? 4 : len;
    uint8_t *src = builder.data_ptr;
    len = builder.number_ptr[1];
    if (len == 0) {
        len = 4;
        memcpy(dst, &MolDefault_Spawns, len);
    } else {
        mol_num_t of = builder.number_ptr[0];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[3];
    if (len == 0) {
        len = 4;
        memcpy(dst, &MolDefault_Pipes, len);
    } else {
        mol_num_t of = builder.number_ptr[2];
        memcpy(dst, src+of, len);
    }
    dst += len;
    len = builder.number_ptr[5];
    if (len == 0) {
        len = 4;
        memcpy(dst, &MolDefault_Writes, len);
    } else {
        mol_num_t of = builder.number_ptr[4];
        memcpy(dst, src+of, len);
    }
    dst += len;
    mol_builder_discard(builder);
    return res;
}

#ifdef __DEFINE_MOLECULE_API_DECORATOR_SPAWN_DAG
#undef MOLECULE_API_DECORATOR
#undef __DEFINE_MOLECULE_API_DECORATOR_SPAWN_DAG
#endif /* __DEFINE_MOLECULE_API_DECORATOR_SPAWN_DAG */

#ifdef __cplusplus
}
#endif /* __cplusplus */

#endif /* SPAWN_DAG_H */
