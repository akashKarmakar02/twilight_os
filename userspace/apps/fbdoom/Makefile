CC ?= musl-gcc
CFLAGS ?= -O2 -static -Wall -DNORMALUNIX -DLINUX -DSNDSERV -D_DEFAULT_SOURCE

DOOM_DIR := ../../doomgeneric/doomgeneric

.PHONY: all clean

all:
	$(MAKE) -C $(DOOM_DIR) -f Makefile.twilight CC="$(CC)"
	cp $(DOOM_DIR)/fbdoom .

clean:
	$(MAKE) -C $(DOOM_DIR) -f Makefile.twilight clean
	rm -f fbdoom
