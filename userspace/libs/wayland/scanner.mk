# Shared native scanner selection for apps that generate Wayland bindings.
# Include after the application's first target to preserve its default goal.
WAYLAND_PORT_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
WAYLAND_SOURCE_REVISION := $(shell git -C "$(WAYLAND_PORT_DIR)/upstream" rev-parse --verify HEAD 2>/dev/null)
PINNED_WAYLAND_SCANNER := $(WAYLAND_PORT_DIR)/build/$(WAYLAND_SOURCE_REVISION)/host/bin/wayland-scanner
WAYLAND_SCANNER ?= $(PINNED_WAYLAND_SCANNER)

.PHONY: wayland-scanner-ready

# Direct app builds can initialize the tool without building guest libraries.
# A supplied WAYLAND_SCANNER override remains the caller's responsibility.
wayland-scanner-ready:
ifeq ($(WAYLAND_SCANNER),$(PINNED_WAYLAND_SCANNER))
	@test -x "$(PINNED_WAYLAND_SCANNER)" || $(MAKE) -C "$(WAYLAND_PORT_DIR)" native-scanner
endif
