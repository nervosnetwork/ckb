BUILDER_DOCKER=nervos/ckb-riscv-gnu-toolchain:bionic-20210804

all:
	riscv64-unknown-elf-gcc -o exec_callee exec_callee.c
	riscv64-unknown-elf-gcc -o exec_caller exec_caller.c

all-via-docker:
	docker run --rm -v `pwd`:/code ${BUILDER_DOCKER} bash -c "cd /code && make all"

.PHONY: all all-via-docker
