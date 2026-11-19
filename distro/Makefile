# FlameDE ISO pipeline. Host-side targets; the ISO itself builds rootless in podman.
HERE    := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
BUILD   := $(HERE)/build
ROOT    := $(abspath $(HERE)/..)
WORK    ?= $(ROOT)/.target/distro/work
SCRIPTS := $(BUILD)/build.sh $(BUILD)/build-iso.sh

.PHONY: deb image iso test clean

deb:
	$(BUILD)/build.sh deb

image:
	$(BUILD)/build.sh image

iso:
	$(BUILD)/build.sh iso

test:
	bash -n $(SCRIPTS)
	@if command -v shellcheck >/dev/null 2>&1; then shellcheck $(SCRIPTS); else echo "shellcheck not installed; lint skipped"; fi

clean:
	@if [ -d "$(WORK)" ]; then podman unshare rm -rf "$(WORK)"; fi
	rm -rf $(HERE)/out
