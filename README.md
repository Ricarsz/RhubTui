# RhubTui

终端里的 2.4G 键鼠电量与 USB 外设面板。Rust + [ratatui](https://ratatui.rs/)，
直接读 `sysfs` / `hidraw`，存储操作交给 `udisks2`。

## 特性

- **设备总览**：按「存储 / 键鼠 HID / 音频 / 网络 / 扩展坞与 Hub / 其他」分组，
  后台扫描 + 热插拔秒级刷新；`/` 过滤，`Tab` 跳分类
- **一眼概览**：详情默认只显示设备类型、连接方式、电量、DPI 档位/回报率与位置，
  `a` 可切到完整的技术详情（控制器、根总线、接口、HID pages……）
- **物理归属**：区分主板内置、根端口直连、经 Hub、经扩展坞；显示 PCI 控制器、
  根总线/根端口、Hub 层级和端口号，并给出判定依据
- **完整信息**：VID:PID、bcdDevice、USB 版本/速率/供电、序列号、厂商/型号、
  设备类；每个 USB 接口的功能、驱动与端点数；多 HID 接口、ALSA 声卡与 PCM、
  USB 网卡、摄像头、串口、打印机；存储的分区表、文件系统、卷标、UUID、
  挂载点与空间占用
- **电量**：MCHOSE 鼠标（`0xFF01`）、MCHOSE 键盘（`0xFF31`）、标准 HID 电量通道；
  鼠标状态包同时解析连接方式（2.4G / USB）与 DPI 档位；失败自动退避重试
- **存储**：`m` 挂载、`u` 卸载、`e` 安全弹出（弹确认框），无分区表的盘直接挂整盘
- **CLI**：`list` / `battery` / `mount` / `umount` / `eject` / `probe` / `doctor` /
  `config`，除 `probe` 外支持 `--json`
- **主题与鼠标**：ocean / forest / sunset；滚轮滚动、左键选择
- **只读安全**：没有已知协议的设备不会盲发命令

## 快速开始

```bash
make pkg
paru -U packaging/aur/rhubtui-<version>-1-x86_64.pkg.tar.zst
rhubtui
```

包内已经带好 udev 规则，装完重新插拔一次接收器即可读电量。

手动安装与可选配置见 [安装](docs/INSTALL.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [安装](docs/INSTALL.md) | 本地打包、PKGBUILD、手动安装、依赖、卸载 |
| [命令与快捷键](docs/COMMANDS.md) | CLI 子命令、JSON 输出、TUI 按键 |
| [配置与限制](docs/CONFIG.md) | 配置文件、拓扑/扩展坞判定、权限、已知限制 |
| [协议](docs/PROTOCOL.md) | MCHOSE 鼠标/键盘与标准 HID 电量协议 |

## 许可

MIT
