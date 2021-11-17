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

# Tip: add `-DDEBUG` to enable the debug outputs
COMMON_CFLAGS := -O3 \
                 -I deps/ckb-c-stdlib \
                 -I deps/ckb-c-stdlib/molecule \
                 -Wall -Werror -g
BIN_CFLAGS := $(COMMON_CFLAGS)
LIB_CFLAGS := -shared -fPIC -nostdlib -nostartfiles -fvisibility=hidden $(COMMON_CFLAGS)

COMMON_LDFLAGS := -Wl,-static -fdata-sections -ffunction-sections -Wl,--gc-sections
BIN_LDFLAGS := $(COMMON_LDFLAGS)
LIB_LDFLAGS := $(COMMON_LDFLAGS)

OBJCOPY_FLAGS := --strip-debug --strip-all

# nervos/ckb-riscv-gnu-toolchain:bionic-20210804
BIN_BUILDER_DOCKER := nervos/ckb-riscv-gnu-toolchain@sha256:cfeb97864cf2039a0900bfa83c3f112a780b2281bded395604b8a8a693c95e08
# nervos/ckb-riscv-gnu-toolchain:gnu-bionic-20210727
LIB_BUILDER_DOCKER := nervos/ckb-riscv-gnu-toolchain@sha256:2341ea4e98836b40c382268201fabd3eff092cb0ecff0746918af38dd22cdd1b

# TODO How to build other binaries?
# Such as `always_success`, `always_failure`, `verify` and `defected_binary`.
ALL_BINS := jalr_zero \
            cpop_lock \
            mop_adc_lock \
            current_cycles \
            vm_version \
            exec_callee \
            exec_caller_from_cell_data \
            exec_caller_from_witness \
            exec_caller_big_offset_length \
            load_is_even \
            load_arithmetic
ALL_LIBS := is_even.lib \
            add1.lib sub1.lib mul2.lib div2.lib

all-bins: clean-bins $(ALL_BINS)
all-libs: clean-libs $(ALL_LIBS)

bins-in-docker:
	docker run --rm -v `pwd`:/code $(BIN_BUILDER_DOCKER) bash -c "cd /code && make all-bins"
libs-in-docker:
	docker run --rm -v `pwd`:/code $(LIB_BUILDER_DOCKER) bash -c "cd /code && make all-libs"

all-in-docker: bins-in-docker libs-in-docker

clean-bins:
	-rm -f $(ALL_BINS)

clean-libs:
	-rm -f $(ALL_LIBS)

clean: clean-bins clean-libs

%: %.c
	$(BIN_CC) $(BIN_CFLAGS) $(BIN_LDFLAGS) -o $@ $<
	$(BIN_OBJCOPY) $(OBJCOPY_FLAGS) $@

%.lib: %.c
	$(LIB_CC) $(LIB_CFLAGS) $(LIB_LDFLAGS) -o $@ $<
	$(LIB_OBJCOPY) $(OBJCOPY_FLAGS) $@

%: %.S
	$(BIN_AS) -o $@.o $<
	$(BIN_LD) -o $@ $@.o
	@rm $@.o
	$(BIN_OBJCOPY) $(OBJCOPY_FLAGS) $@

jalr_zero: jalr_zero.S
cpop_lock: cpop_lock.c
mop_adc_lock: mop_adc_lock.S
current_cycles: current_cycles.c
vm_version: vm_version.c
exec_callee: exec_callee.c
exec_caller_from_cell_data: exec_caller_from_cell_data.c
exec_caller_from_witness: exec_caller_from_witness.c
exec_caller_big_offset_length: exec_caller_big_offset_length.c
load_is_even: load_is_even.c
is_even.lib: is_even.c
add1.lib: add1.c
sub1.lib: sub1.c
mul2.lib: mul2.c
div2.lib: div2.c
load_arithmetic: load_arithmetic.c
