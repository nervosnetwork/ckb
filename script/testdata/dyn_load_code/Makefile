# for shared library: -shared not supported
BIN_TARGET := riscv64-unknown-elf
BIN_CC :=      $(BIN_TARGET)-gcc
BIN_LD :=      $(BIN_TARGET)-gcc
BIN_OBJCOPY := $(BIN_TARGET)-objcopy
# for binary: cannot find -lgcc_s
LIB_TARGET := riscv64-unknown-linux-gnu
LIB_CC :=      $(LIB_TARGET)-gcc
LIB_LD :=      $(LIB_TARGET)-gcc
LIB_OBJCOPY := $(LIB_TARGET)-objcopy

COMMON_CFLAGS := -O3 -I ../deps/ckb-c-stdlib \
                 -I ../deps/molecule \
                 -I ../c \
                 -Wall -Werror -g
BIN_CFLAGS := $(COMMON_CFLAGS)
LIB_CFLAGS := -shared -fPIC -nostdlib -nostartfiles -fvisibility=hidden $(COMMON_CFLAGS)

COMMON_LDFLAGS := -Wl,-static -fdata-sections -ffunction-sections -Wl,--gc-sections
BIN_LDFLAGS := $(COMMON_LDFLAGS)
LIB_LDFLAGS := $(COMMON_LDFLAGS)

# nervos/ckb-riscv-gnu-toolchain:bionic-20190702
BIN_BUILDER_DOCKER := nervos/ckb-riscv-gnu-toolchain@sha256:7b168b4b109a0f741078a71b7c4dddaf1d283a5244608f7851f5714fbad273ba
# nervos/ckb-riscv-gnu-toolchain:gnu-bionic-20191012
LIB_BUILDER_DOCKER := nervos/ckb-riscv-gnu-toolchain@sha256:aae8a3f79705f67d505d1f1d5ddc694a4fd537ed1c7e9622420a470d59ba2ec3

all-bin: dyn_load_code_lock
all-lib: dyn_load_code_lib

bin-in-docker:
	docker run --rm -v `cd ../; pwd`:/code ${BIN_BUILDER_DOCKER} bash -c "cd /code/dyn_load_code && make all-bin"
lib-in-docker:
	docker run --rm -v `cd ../; pwd`:/code ${LIB_BUILDER_DOCKER} bash -c "cd /code/dyn_load_code && make all-lib"

all-in-docker: clean bin-in-docker lib-in-docker

dyn_load_code_lib: dyn_load_code_lib.c
	$(LIB_CC) $(LIB_CFLAGS) $(LIB_LDFLAGS) -o $@ $<
	$(LIB_OBJCOPY) --only-keep-debug $@ $@.debug
	$(LIB_OBJCOPY) --strip-debug --strip-all $@

dyn_load_code_lock: dyn_load_code_lock.c
	$(BIN_CC) $(BIN_CFLAGS) $(BIN_LDFLAGS) -o $@ $<
	$(BIN_OBJCOPY) --only-keep-debug $@ $@.debug
	$(BIN_OBJCOPY) --strip-debug --strip-all $@

clean:
	-rm -f dyn_load_code_lib dyn_load_code_lock *.debug
