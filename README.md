# RhubTui

终端里的 USB 设备查看器与移动盘挂载工具。Rust + [ratatui](https://ratatui.rs/)，
直接读 `sysfs`/`udev` 信息，存储操作交给 `udisks2`。

## 定位

RhubTui 只做两件事：

1. **看清插了什么**：USB 设备、接口、拓扑、物理端口，鼠标键盘也只识别身份，不碰配置。
2. **安全操作移动盘**：挂载、卸载、弹出 U 盘与移动硬盘，带确认框。

它不会向键鼠发送厂商命令，也不读电量或改 DPI——设备配置以厂商驱动为准。

## 特性

- **设备总览**：按「存储 / 键鼠 HID / 音频 / 扩展坞与 Hub / 其他」分组，
  后台扫描 + 热插拔秒级刷新；`/` 过滤，`Tab` 跳分类
- **插入即识别**：名称、VID:PID、序列号、USB 速率、接口与驱动、声卡、摄像头
- **物理归属**：区分主板内置、根端口直连、经 Hub、经扩展坞，并给出具体端口
- **一眼概览**：详情最多 6 行，只看人类关心的信息，不堆技术参数
- **移动盘**：`m` 挂载、`u` 卸载、`e` 安全弹出（弹确认框），无分区表的盘直接挂整盘
- **只读安全**：不打开 hidraw、不向设备写入任何数据
- **CLI**：`list` / `mount` / `umount` / `eject` / `doctor` / `config`，均支持 `--json`
- **主题与鼠标**：ocean / forest / sunset；滚轮滚动、左键选择

## 快速开始

```bash
make pkg
paru -U packaging/aur/rhubtui-<version>-1-x86_64.pkg.tar.zst
rhubtui
```

挂载依赖 `udisks2`；无需额外 udev 规则。手动安装见 [安装](docs/INSTALL.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [安装](docs/INSTALL.md) | 本地打包、PKGBUILD、手动安装、依赖、卸载 |
| [命令与快捷键](docs/COMMANDS.md) | CLI 子命令、JSON 输出、TUI 按键 |
| [配置与限制](docs/CONFIG.md) | 配置文件、拓扑/扩展坞判定、已知限制 |

## 许可

MIT
