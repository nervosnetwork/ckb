# for shared library: -shared not supported
BIN_TARGET := riscv64-unknown-elf
BIN_CC :=      $(BIN_TARGET)-gcc
BIN_LD :=      $(BIN_TARGET)-ld
BIN_OBJCOPY := $(BIN_TARGET)-objcopy
BIN_AS :=      $(BIN_TARGET)-as
# for binary: cannot find -lgcc_s
LIB_TARGET := riscv64-unknown-linux-gnu
LIB_CC :=      $(LIB_TARGET)-gcc
LIB_OBJCOPY := $(LIB_TARGET)-objcopy

COMMON_CFLAGS := -O3 \
                 -I deps/ckb-c-stdlib \
                 -I deps/ckb-c-stdlib/libc \
                 -I deps/ckb-c-stdlib/molecule \
                 -Wall -Werror \
				 -fno-builtin -nostdinc -nostartfiles \
				 -Wno-stringop-overflow
# enable log
COMMON_CFLAGS += -DCKB_C_STDLIB_PRINTF -DCKB_C_STDLIB_PRINTF_BUFFER_SIZE=1024
# enable debug
# COMMON_CFLAGS += -g

BIN_CFLAGS := $(COMMON_CFLAGS)
LIB_CFLAGS := -shared -fPIC -nostdlib -nostartfiles -fvisibility=hidden $(COMMON_CFLAGS)

COMMON_LDFLAGS := -Wl,-static -fdata-sections -ffunction-sections -Wl,--gc-sections
BIN_LDFLAGS := $(COMMON_LDFLAGS)
LIB_LDFLAGS := $(COMMON_LDFLAGS)

# nervos/ckb-riscv-gnu-toolchain:bionic-20210804
BIN_BUILDER_DOCKER := nervos/ckb-riscv-gnu-toolchain@sha256:cfeb97864cf2039a0900bfa83c3f112a780b2281bded395604b8a8a693c95e08
# nervos/ckb-riscv-gnu-toolchain:gnu-bionic-20210727
LIB_BUILDER_DOCKER := nervos/ckb-riscv-gnu-toolchain@sha256:2341ea4e98836b40c382268201fabd3eff092cb0ecff0746918af38dd22cdd1b

# TODO How to build other binaries?
# Such as `always_success`, `always_failure`, `verify` and `defected_binary`.
ALL_BINS := jalr_zero \
            cadd_hint_lock \
            cpop_lock \
            mop_adc_lock \
            current_cycles \
            current_cycles_with_snapshot \
            vm_version \
            vm_version_2 \
            vm_version_with_snapshot \
            exec_callee \
            exec_caller_from_cell_data \
            exec_caller_from_witness \
            exec_caller_big_offset_length \
            exec_configurable_callee \
            exec_configurable_caller \
            infinite_loop \
            load_code_to_stack_then_reuse \
            load_is_even_into_global \
            load_is_even_with_snapshot \
            load_arithmetic \
            debugger \
			spawn_caller_strcat \
			spawn_callee_strcat
			
			            
ALL_LIBS := is_even.lib \
            add1.lib sub1.lib mul2.lib div2.lib

all-bins: $(ALL_BINS)
all-libs: $(ALL_LIBS)

bins-in-docker:
	docker run --rm -v `pwd`:/code $(BIN_BUILDER_DOCKER) bash -c "cd /code && make all-bins"
libs-in-docker:
	docker run --rm -v `pwd`:/code $(LIB_BUILDER_DOCKER) bash -c "cd /code && make all-libs"

all-in-docker: bins-in-docker libs-in-docker

clean-bins:
	-rm -f $(ALL_BINS)

clean-libs:
	-rm -f $(ALL_LIBS)

fmt:
	clang-format -i *.c *.h

clean: clean-bins clean-libs

%: %.c
	$(BIN_CC) $(BIN_CFLAGS) $(BIN_LDFLAGS) -o $@ $<

%.lib: %.c
	$(LIB_CC) $(LIB_CFLAGS) $(LIB_LDFLAGS) -o $@ $<

%: %.S
	$(BIN_AS) -o $@.o $<
	$(BIN_LD) -o $@ $@.o
	@rm $@.o

jalr_zero: jalr_zero.S
cadd_hint_lock: cadd_hint_lock.S
	$(BIN_AS) -march=rv64imc -o $@.o $<
	$(BIN_LD) -o $@ $@.o
	@rm $@.o
cpop_lock: cpop_lock.c
mop_adc_lock: mop_adc_lock.S
current_cycles: current_cycles.c
current_cycles_with_snapshot: current_cycles_with_snapshot.c
infinite_loop: infinite_loop.c
vm_version: vm_version.c
vm_version_2: vm_version_2.c
vm_version_with_snapshot: vm_version_with_snapshot.c
exec_callee: exec_callee.c
exec_caller_from_cell_data: exec_caller_from_cell_data.c
exec_caller_from_witness: exec_caller_from_witness.c
exec_caller_big_offset_length: exec_caller_big_offset_length.c
exec_configurable_callee: exec_configurable_callee.c
exec_configurable_caller: exec_configurable_caller.c
load_code_to_stack_then_reuse: load_code_to_stack_then_reuse.c
load_is_even_into_global: load_is_even_into_global.c
load_is_even_with_snapshot: load_is_even_with_snapshot.c
is_even.lib: is_even.c
add1.lib: add1.c
sub1.lib: sub1.c
mul2.lib: mul2.c
div2.lib: div2.c
load_arithmetic: load_arithmetic.c

spawn_caller_strcat: spawn_caller_strcat.c utils.h
spawn_callee_strcat: spawn_callee_strcat.c utils.h

