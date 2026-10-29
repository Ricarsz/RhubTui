# 安装

包名：`rhubtui`  
安装后的命令：`rhubtui`

包内容：

| 路径 | 说明 |
|---|---|
| `/usr/bin/rhubtui` | 可执行文件 |
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

## 配置（可选）

```bash
rhubtui config --init          # 写入默认配置
rhubtui config                 # 查看路径和当前值
```

配置项见 [配置与限制](CONFIG.md)。不创建配置文件也能正常使用。

## 权限

- **挂载**：走 udisks2 的 D-Bus 接口，普通 U 盘在活动本地会话里免密；
  个别 USB 硬盘盒会被判成不可移动设备，需要 polkit 授权。

## 卸载

```bash
sudo pacman -R rhubtui
# 手动安装：
sudo make uninstall
```
