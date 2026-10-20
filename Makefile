PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
CARGO ?= cargo
VERSION ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)

.PHONY: all build install uninstall test check fmt pkg dist clean

all: build

build:
	$(CARGO) build --release

install: build
	install -Dm755 target/release/rhubtui $(DESTDIR)$(BINDIR)/rhubtui

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/rhubtui

test:
	$(CARGO) test

check:
	$(CARGO) clippy --all-targets -- -D warnings

fmt:
	$(CARGO) fmt

# 生成 PKGBUILD 使用的源码 tarball（需要已提交的 git 仓库）。
dist:
	git archive --format=tar.gz --prefix=rhubtui-$(VERSION)/ \
		-o packaging/aur/rhubtui-$(VERSION).tar.gz HEAD

# 构建 pacman 包；产物用 paru -U 安装。
pkg: dist
	cd packaging/aur && makepkg -f --noconfirm
	@ls -lh packaging/aur/*.pkg.tar.zst

clean:
	$(CARGO) clean
	rm -rf packaging/aur/src packaging/aur/pkg \
		packaging/aur/*.pkg.tar.zst packaging/aur/*.tar.gz
