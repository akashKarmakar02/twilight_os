CC ?= musl-gcc
CFLAGS ?= -O2 -Wall -Wextra -static -fno-pie -no-pie

.PHONY: all clean

all: twland_shm_client

twland_shm_client: twland_shm_client.c
	$(CC) $(CFLAGS) $< -o $@

clean:
	rm -f twland_shm_client
