# 安装

包名：`rhubtui`  
安装后的命令：`rhubtui`

包内容：

| 路径 | 说明 |
|---|---|
| `/usr/bin/rhubtui` | 可执行文件 |
| `/usr/lib/udev/rules.d/70-rhubtui.rules` | MCHOSE 接收器的 `hidraw` 权限 |
| `/usr/share/doc/rhubtui/` | README 与 docs |

依赖：`udisks2`（挂载/弹出）、`util-linux`（`lsblk`）。

## 本地打包安装

在仓库根目录：

```bash
make pkg
paru -U packaging/aur/rhubtui-<version>-1-x86_64.pkg.tar.zst
```

`make pkg` 会先用 `git archive` 生成源码 tarball，再调用 `makepkg`。只构建不安装：

```bash
make pkg
ls packaging/aur/*.pkg.tar.zst
```

## 用 PKGBUILD 直接安装

```bash
make dist
cd packaging/aur
paru -Bi          # 等价于 makepkg -si
```

`make dist` 需要在已提交的 git 仓库里运行。

## 手动构建

```bash
make              # cargo build --release
sudo make install # /usr/local/bin/rhubtui
```

手动安装时 udev 规则需要自己放：

```bash
sudo install -Dm644 udev/70-rhubtui.rules \
  /usr/lib/udev/rules.d/70-rhubtui.rules
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=hidraw
```

重新插拔 2.4G 接收器后，电量读取才有权限。

## 权限

- **电量**：需要 `/dev/hidraw*` 读写权限，由 udev 规则给当前登录会话打 `uaccess`。
  没有规则时 `doctor` 会报 `hidraw 权限 ❌`，TUI 详情也会提示。
- **挂载**：走 udisks2 的 D-Bus 接口，普通 U 盘在活动本地会话里免密；
  个别 USB 硬盘盒会被判成不可移动设备，需要 polkit 授权。

## 卸载

```bash
sudo pacman -R rhubtui
# 手动安装：
sudo make uninstall
```
