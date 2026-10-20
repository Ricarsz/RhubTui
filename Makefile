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

# 生成 PKGBUILD 使用的源码 tarball。
# 有 git 时用 git archive（只含已提交内容）；否则退回 tar 打包工作区，
# 这样在容器/CI 里没有 .git 也能构建。
dist:
	@mkdir -p packaging/aur
	@if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then \
		git archive --format=tar.gz --prefix=rhubtui-$(VERSION)/ \
			-o packaging/aur/rhubtui-$(VERSION).tar.gz HEAD; \
	else \
		tar --exclude=.git --exclude=target \
			--exclude=packaging/aur/src --exclude=packaging/aur/pkg \
			--exclude='packaging/aur/*.tar.gz' \
			--exclude='packaging/aur/*.pkg.tar.zst' \
			--transform='s,^\./,rhubtui-$(VERSION)/,' \
			-czf packaging/aur/rhubtui-$(VERSION).tar.gz .; \
	fi

# 构建 pacman 包；产物用 paru -U 安装。
pkg: dist
	cd packaging/aur && makepkg -f --noconfirm
	@ls -lh packaging/aur/*.pkg.tar.zst

clean:
	$(CARGO) clean
	rm -rf packaging/aur/src packaging/aur/pkg \
		packaging/aur/*.pkg.tar.zst packaging/aur/*.tar.gz
