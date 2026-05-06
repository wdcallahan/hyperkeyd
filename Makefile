PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin

.PHONY: build check test install clean

build:
	cargo build --release

check:
	cargo check

test:
	cargo test

install: build
	install -Dm755 target/release/hyperkeyd $(BINDIR)/hyperkeyd

clean:
	cargo clean
