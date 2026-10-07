# Installs for the current user by default (no sudo needed).
#   make && make install
#   make && sudo make install PREFIX=/usr/local      # system wide
#   make install DESTDIR="$pkgdir" PREFIX=/usr       # packaging
#   make static && make install-static               # fully static binary (musl)
#   make dist                                        # release tarball in dist/
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man
CARGO ?= cargo

# Static builds: needs the musl Rust target (`rustup target add
# x86_64-unknown-linux-musl`, or the `rust-musl` package on Arch) and
# musl-gcc for the bundled SQLite (`musl` / `musl-tools` package).
MUSL_TARGET ?= x86_64-unknown-linux-musl

BIN := target/release/sshh
STATIC_BIN := target/$(MUSL_TARGET)/release/sshh
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml 2>/dev/null | head -n 1)
DIST_NAME := sshh-$(VERSION)-$(MUSL_TARGET)

.PHONY: all build static install install-static install-files uninstall dist man lint-man test

all: build

build:
	$(CARGO) build --release --locked

static:
	$(CARGO) build --release --locked --target $(MUSL_TARGET)

# Doesn't build, so `sudo make install` never runs cargo as root.
install:
	@$(MAKE) --no-print-directory install-files SRC_BIN=$(BIN)

install-static:
	@$(MAKE) --no-print-directory install-files SRC_BIN=$(STATIC_BIN) BUILD_HINT=static

install-files:
	@test -x $(SRC_BIN) || { echo "$(SRC_BIN) not found: run 'make $(BUILD_HINT)' first" >&2; exit 1; }
	install -Dm755 $(SRC_BIN) $(DESTDIR)$(BINDIR)/sshh
	install -Dm644 man/sshh.1 $(DESTDIR)$(MANDIR)/man1/sshh.1

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/sshh $(DESTDIR)$(MANDIR)/man1/sshh.1

# Release tarball with the static binary: dist/sshh-<version>-<target>.tar.gz
# (+ .sha256). Unpack it and copy `sshh` to your PATH and `sshh.1` to man1.
dist: static
	rm -rf dist/$(DIST_NAME)
	install -Dm755 $(STATIC_BIN) dist/$(DIST_NAME)/sshh
	install -Dm644 man/sshh.1 dist/$(DIST_NAME)/sshh.1
	install -Dm644 README.md dist/$(DIST_NAME)/README.md
	tar -C dist -czf dist/$(DIST_NAME).tar.gz $(DIST_NAME)
	cd dist && sha256sum $(DIST_NAME).tar.gz > $(DIST_NAME).tar.gz.sha256
	rm -rf dist/$(DIST_NAME)
	@echo "dist/$(DIST_NAME).tar.gz"

# Preview the man page without installing it.
man:
	man -l man/sshh.1

lint-man:
	groff -man -ww -z man/sshh.1

test:
	$(CARGO) test
