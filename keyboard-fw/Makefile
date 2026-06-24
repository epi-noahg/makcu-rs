# Host unit-test runner for the portable keyboard-firmware core.
#
#   make test      build + run every test/test_*.c
#   make clean
#
# Each test links against all of core/*.c (none of which define main()).
# Needs only a C11 compiler — no PlatformIO, no hardware.

CC      ?= cc
CFLAGS  ?= -std=c11 -Wall -Wextra -Werror -g -Icore -Itest
BUILD   := build

# km_glue.c needs platform hooks (km_now_ms / km_uart_write_raw / km_platform_start),
# so it is only linked into test_glue (which stubs them). All other tests link
# the hook-free base core.
GLUE      := core/km_glue.c
CORE_BASE := $(filter-out $(GLUE),$(wildcard core/*.c))
TESTS     := $(wildcard test/test_*.c)
BINS      := $(patsubst test/%.c,$(BUILD)/%,$(TESTS))

.PHONY: all test clean
all: test

$(BUILD):
	@mkdir -p $(BUILD)

# Explicit rule (takes precedence over the pattern rule below).
$(BUILD)/test_glue: test/test_glue.c $(CORE_BASE) $(GLUE) test/test_framework.h | $(BUILD)
	$(CC) $(CFLAGS) test/test_glue.c $(CORE_BASE) $(GLUE) -o $@

$(BUILD)/%: test/%.c $(CORE_BASE) test/test_framework.h | $(BUILD)
	$(CC) $(CFLAGS) $< $(CORE_BASE) -o $@

test: $(BINS)
	@fail=0; \
	for b in $(BINS); do \
		echo "== $$b =="; \
		./$$b || fail=1; \
	done; \
	if [ $$fail -ne 0 ]; then echo "\nSOME TESTS FAILED"; exit 1; fi; \
	echo "\nALL TESTS PASSED"

clean:
	rm -rf $(BUILD)
