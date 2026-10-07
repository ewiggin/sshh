# Installs for the current user by default (no sudo needed).
#   make && make install
#   make && sudo make install PREFIX=/usr/local      # system wide
#   make install DESTDIR="$pkgdir" PREFIX=/usr       # packaging
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man
CARGO ?= cargo

BIN := target/release/sshh

.PHONY: all build install uninstall man lint-man test

all: build

build:
	$(CARGO) build --release --locked

# Doesn't build, so `sudo make install` never runs cargo as root.
install:
	@test -x $(BIN) || { echo "$(BIN) not found: run 'make' first" >&2; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(BINDIR)/sshh
	install -Dm644 man/sshh.1 $(DESTDIR)$(MANDIR)/man1/sshh.1

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/sshh $(DESTDIR)$(MANDIR)/man1/sshh.1

# Preview the man page without installing it.
man:
	man -l man/sshh.1

lint-man:
	groff -man -ww -z man/sshh.1

test:
	$(CARGO) test
