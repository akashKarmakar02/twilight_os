CC ?= musl-gcc
CFLAGS ?= -O2 -static -Wall -Wextra

unix_bind_test: unix_bind_test.c
	$(CC) $(CFLAGS) -o $@ $<

.PHONY: clean
clean:
	rm -f unix_bind_test
