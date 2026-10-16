# Plain `make` lists the targets (`make help`). Installs for the current user
# by default (no sudo needed).
#   make build && make install
#   make build && sudo make install PREFIX=/usr/local  # system wide
#   make install DESTDIR="$pkgdir" PREFIX=/usr       # packaging
#   make static && make install-static               # fully static binary (musl)
#   make dist                                        # release tarball in dist/
#   make dist MUSL_TARGET=aarch64-unknown-linux-musl # (on an aarch64 machine)
#   make dist                                        # on macOS: aarch64-apple-darwin
#   make version v0.2.0 (or patch|minor|major)       # bump the version and commit
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man
CARGO ?= cargo

# Static builds: needs the musl Rust target (`rustup target add
# x86_64-unknown-linux-musl`, or the `rust-musl` package on Arch) and
# musl-gcc for the bundled SQLite (`musl` / `musl-tools` package).
MUSL_TARGET ?= x86_64-unknown-linux-musl

# Target of `make dist`: the static musl build on Linux; on macOS the native
# build (it only links system libraries, SQLite is bundled).
ifeq ($(shell uname -s),Darwin)
DIST_TARGET ?= $(shell uname -m | sed 's/arm64/aarch64/')-apple-darwin
else
DIST_TARGET ?= $(MUSL_TARGET)
endif

BIN := target/release/sshh
STATIC_BIN := target/$(MUSL_TARGET)/release/sshh
DIST_BIN := target/$(DIST_TARGET)/release/sshh
# No version in the name, so .../releases/latest/download/<name> always works.
DIST_NAME := sshh-$(DIST_TARGET)

.PHONY: help build static install install-static install-files uninstall dist version man lint-man test
.DEFAULT_GOAL := help

# Targets are documented with a `## ` comment after their name; `help` prints them.
help: ## Show this help
	@echo "Usage: make <target> [VARIABLE=value]"
	@echo
	@echo "Targets:"
	@awk -F ':.*## ' '/^[a-z-]+:.*## / { printf "  %-15s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo
	@echo "Variables:"
	@echo "  PREFIX          where to install (now: $(PREFIX))"
	@echo "  DESTDIR         staging root prepended to PREFIX, for packaging"
	@echo "  MUSL_TARGET     target of the static build (now: $(MUSL_TARGET))"
	@echo "  DIST_TARGET     target of the release tarball (now: $(DIST_TARGET))"
	@echo "  CARGO           cargo binary to use (now: $(CARGO))"
	@echo
	@echo "Typical use: make build && make install"

build: ## Build the release binary (target/release/sshh)
	$(CARGO) build --release --locked

static: ## Build a fully static binary (musl; needs the musl target and musl-gcc)
	$(CARGO) build --release --locked --target $(MUSL_TARGET)

# Doesn't build, so `sudo make install` never runs cargo as root.
install: ## Install the binary of 'make build' and the man page under PREFIX
	@$(MAKE) --no-print-directory install-files SRC_BIN=$(BIN) BUILD_HINT=build

install-static: ## Install the binary of 'make static' and the man page under PREFIX
	@$(MAKE) --no-print-directory install-files SRC_BIN=$(STATIC_BIN) BUILD_HINT=static

install-files:
	@test -x $(SRC_BIN) || { echo "$(SRC_BIN) not found: run 'make $(BUILD_HINT)' first" >&2; exit 1; }
	install -d $(DESTDIR)$(BINDIR) $(DESTDIR)$(MANDIR)/man1
	install -m755 $(SRC_BIN) $(DESTDIR)$(BINDIR)/sshh
	install -m644 man/sshh.1 $(DESTDIR)$(MANDIR)/man1/sshh.1

uninstall: ## Remove the installed binary and man page (same PREFIX/DESTDIR)
	rm -f $(DESTDIR)$(BINDIR)/sshh $(DESTDIR)$(MANDIR)/man1/sshh.1

# Release tarball: dist/sshh-<target>.tar.gz (+ .sha256), with the static binary
# on Linux. Unpack it and copy `sshh` to your PATH and `sshh.1` to man1
# (install.sh does it). `install`, `tar` and `shasum` work with GNU and BSD tools.
dist: ## Build the release tarball in dist/ (static binary on Linux; + .sha256)
	$(CARGO) build --release --locked --target $(DIST_TARGET)
	rm -rf dist/$(DIST_NAME)
	install -d dist/$(DIST_NAME)
	install -m755 $(DIST_BIN) dist/$(DIST_NAME)/sshh
	install -m644 man/sshh.1 dist/$(DIST_NAME)/sshh.1
	install -m644 README.md LICENSE-MIT LICENSE-APACHE dist/$(DIST_NAME)
	COPYFILE_DISABLE=1 tar -C dist -czf dist/$(DIST_NAME).tar.gz $(DIST_NAME)
	cd dist && { sha256sum $(DIST_NAME).tar.gz 2>/dev/null || shasum -a 256 $(DIST_NAME).tar.gz; } > $(DIST_NAME).tar.gz.sha256
	rm -rf dist/$(DIST_NAME)
	@echo "dist/$(DIST_NAME).tar.gz"

# `make version v0.2.0`: bump the version (Cargo.toml, Cargo.lock, man page) and
# commit "bump to v0.2.0". The word after `version` is its argument, not a
# target, so it gets an empty rule.
ifeq (version,$(firstword $(MAKECMDGOALS)))
VERSION_ARG := $(word 2,$(MAKECMDGOALS))
ifneq ($(VERSION_ARG),)
$(eval $(VERSION_ARG):;@:)
endif
endif

version: ## Bump the version and commit: make version <vX.Y.Z | patch | minor | major>
	@test -n "$(VERSION_ARG)" || { echo "usage: make version <vX.Y.Z | patch | minor | major>" >&2; exit 1; }
	@scripts/bump.sh $(VERSION_ARG)

# Preview the man page without installing it.
man: ## Preview the man page without installing it
	man ./man/sshh.1

lint-man: ## Check the man page with groff
	groff -man -ww -z man/sshh.1

test: ## Run the tests (cargo test)
	$(CARGO) test
