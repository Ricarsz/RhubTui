# 命令与快捷键

## CLI

不带子命令时启动 TUI。

| 命令 | 说明 |
|---|---|
| `rhubtui list` | 打印一次设备快照（不读电量） |
| `rhubtui battery [设备]` | 读取所有可读设备的电量 |
| `rhubtui mount [设备]` | 挂载分区；无分区时挂整盘 |
| `rhubtui umount [设备]` | 卸载分区或整盘 |
| `rhubtui eject [设备]` | 安全弹出（要求已卸载） |
| `rhubtui probe <节点>` | 抓取 HID 原始报文 |
| `rhubtui doctor` | 检查依赖、udev 规则和 hidraw 权限 |
| `rhubtui config` | 显示配置路径与当前值 |

全局参数：

- `--json`：所有子命令支持，输出 JSON
- `-h, --help` / `-V, --version`

设备选择器可以是：

- 端口：`7-1.4`
- VID:PID：`5253:1020`
- HID 节点：`/dev/hidraw13`（电量）
- 块设备：`/dev/sdc1`、`sdc1`、`sdc`（挂载/卸载/弹出）

示例：

```bash
rhubtui list
rhubtui battery
rhubtui --json battery
rhubtui battery 3837:3033
rhubtui mount sdc1
rhubtui mount            # 第一个未挂载分区
rhubtui umount
rhubtui eject sdc
rhubtui doctor
```

`battery` 的 JSON：

```json
[
  {
    "device": "MCHOSE A7 Pro",
    "port": "7-1.4",
    "node": "/dev/hidraw13",
    "percent": 96,
    "charging": false,
    "polling_hz": 1000
  }
]
```

部分设备读不到时，成功的照常输出，失败信息写到 stderr；全部失败才返回非零。

## TUI 按键

| 按键 | 动作 |
|---|---|
| `↑` `↓` / `k` `j` | 选择设备 |
| `←` `→` / `h` `l` | 选择存储分区 |
| `m` / `u` | 挂载 / 卸载 |
| `e` | 安全弹出（需先卸载） |
| `b` | 重新读取电量 |
| `r` | 立即重新扫描 |
| `PgUp` / `PgDn` | 滚动详情 |
| `?` | 帮助 |
| `q` / `Esc` / `Ctrl+C` | 退出 |

## 抓包

```bash
rhubtui probe /dev/hidraw2 --decode --seconds 15
```

`--decode` 会按 `0xFF` 异或解码 payload（MCHOSE 鼠标协议用），键盘协议不需要。
