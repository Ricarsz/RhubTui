# RhubTui

终端里的 2.4G 键鼠电量与 USB 外设面板。Rust + [ratatui](https://ratatui.rs/)，
直接读 `sysfs` / `hidraw`，存储操作交给 `udisks2`。

## 特性

- **设备总览**：按「存储 / 键鼠 / 音频 / 集线器 / 其他」分组，热插拔秒级刷新
- **电量**：MCHOSE 鼠标（`0xFF01`）、MCHOSE 键盘（`0xFF31`）、标准 HID 电量通道
- **存储**：分区、文件系统、卷标、挂载点；`m` 挂载、`u` 卸载、`e` 安全弹出，
  无分区表的盘直接挂整盘
- **详情**：VID:PID、速率、供电、序列号、USB 接口与驱动、ALSA 声卡
- **CLI**：`list` / `battery` / `mount` / `umount` / `eject` / `probe` / `doctor`，
  全部支持 `--json`
- **主题**：ocean / forest / sunset
- **只读安全**：没有已知协议的设备不会盲发命令

## 快速开始

```bash
make pkg
paru -U packaging/aur/rhubtui-<version>-1-x86_64.pkg.tar.zst
rhubtui
```

包内已经带好 udev 规则，装完重新插拔一次接收器即可读电量。

手动安装见 [安装](docs/INSTALL.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [安装](docs/INSTALL.md) | 本地打包、PKGBUILD、手动安装、依赖、卸载 |
| [命令与快捷键](docs/COMMANDS.md) | CLI 子命令、JSON 输出、TUI 按键 |
| [配置与限制](docs/CONFIG.md) | 配置文件、主题、权限、已知限制 |
| [协议](docs/PROTOCOL.md) | MCHOSE 鼠标/键盘与标准 HID 电量协议 |

## 许可

MIT
